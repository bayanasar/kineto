import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/native/kineto_engine.dart';
import 'package:kineto/voices_workspace.dart';
import 'package:wabisabi/wabisabi.dart';

const KinetoLanguageModel _kimiCode =
    KinetoLanguageModel(kind: KinetoLanguageModelKind.kimiCode, ready: true, model: 'agent-default');

const List<KinetoVoice> _catalog = <KinetoVoice>[
  KinetoVoice(id: 'grave_man', name: 'Grave Man', description: 'Low and slow.'),
  KinetoVoice(id: 'bright_woman', name: 'Bright Woman'),
  KinetoVoice(id: 'calm_woman', name: 'Calm Woman'),
];

const KinetoCharacterVoice _filmmaker = KinetoCharacterVoice(
  characterId: 'filmmaker',
  characterName: 'The Filmmaker',
  defaultLine: 'This is how I sound.',
  auditions: <KinetoAudition>[
    KinetoAudition(
      id: 'voice.filmmaker.0001',
      status: KinetoChoiceStatus.selected,
      voiceId: 'grave_man',
      voiceName: 'Grave Man',
      line: 'This is how I sound.',
      mediaPath: '/film/characters/filmmaker/voices/voice.filmmaker.0001.mp3',
      durationMillis: 1400,
      emotion: 'calm',
    ),
  ],
);

const KinetoCharacterVoice _interviewer = KinetoCharacterVoice(
  characterId: 'interviewer',
  characterName: 'The Interviewer',
  defaultLine: 'Was it worth it?',
  lockedAudition: 'voice.interviewer.0001',
  lockedVoiceId: 'calm_woman',
  lockedVoiceName: 'Calm Woman',
  recommended: <KinetoRecommendedVoice>[
    KinetoRecommendedVoice(voiceId: 'calm_woman', name: 'Calm Woman', reason: 'Even, patient.'),
  ],
);

const KinetoLine _question = KinetoLine(
  sceneId: 'office_night',
  number: 1,
  text: 'Was it worth it?',
  characterId: 'interviewer',
  parenthetical: 'quietly',
  takeCostMicros: 1600,
  durationMillis: 1200,
  takes: <KinetoLineTake>[
    KinetoLineTake(
      id: 'line.office_night.0001',
      status: KinetoChoiceStatus.candidate,
      mediaPath: '/film/scenes/office_night/lines/line.office_night.0001.mp3',
      durationMillis: 1300,
      note: 'Barely said.',
    ),
  ],
);

const KinetoVoices _voices = KinetoVoices(
  speechProvider: KinetoSpeechProviderKind.minimax,
  characterCostMicros: 100,
  voices: _catalog,
  characters: <KinetoCharacterVoice>[_filmmaker, _interviewer],
  lines: <KinetoLine>[_question],
);

final class _Calls {
  final List<String> log = <String>[];
  List<KinetoVoiceChoice>? auditioned;
  DubRequest? dubbed;
}

Future<_Calls> _pump(WidgetTester tester, {KinetoVoices voices = _voices}) async {
  tester.view.physicalSize = const Size(1400, 2400);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  final calls = _Calls();
  await tester.pumpWidget(
    MaterialApp(
      theme: WabTheme.materialTheme(),
      home: VoicesWorkspaceView(
        voices: voices,
        model: _kimiCode,
        // $0.0001 a character.
        speechCost: (text, count) => text.length * 100 * count,
        onPlay: (path) => calls.log.add('play $path'),
        onLoadVoices: () => calls.log.add('load'),
        onRecommend: () => calls.log.add('recommend'),
        onAudition: (character, choices, line) {
          calls.auditioned = choices;
          calls.log.add('audition $character $line');
        },
        onSelectVoice: (character, audition) => calls.log.add('select $character $audition'),
        onRejectVoice: (character, audition) => calls.log.add('reject $character $audition'),
        onLockVoice: (character) => calls.log.add('lock $character'),
        onDub: (line, request) {
          calls.dubbed = request;
          calls.log.add('dub ${line.sceneId} ${line.number}');
        },
        onSelectTake: (line, take) => calls.log.add('select take $take'),
        onRejectTake: (line, take) => calls.log.add('reject take $take'),
        onLockLine: (line) => calls.log.add('lock line ${line.number}'),
      ),
    ),
  );
  return calls;
}

Future<void> _tap(WidgetTester tester, Finder finder) async {
  await tester.ensureVisible(finder);
  await tester.tap(finder, warnIfMissed: false);
  await tester.pump();
}

ValueKey<String> _key(String key) => ValueKey<String>(key);

void main() {
  testWidgets('voices are chosen to audition with their price shown, and the chosen one locked', (tester) async {
    final calls = await _pump(tester);
    expect(find.text(r'$0.1 per 1,000 characters spoken.'), findsOneWidget);
    expect(find.text('3 voices listed.'), findsOneWidget);
    expect(find.text('Voice locked · Calm Woman'), findsOneWidget);
    expect(find.text('Auditioned'), findsOneWidget);
    expect(find.text('Grave Man · calm · 1.4 s · “This is how I sound.”'), findsOneWidget);

    // Nothing chosen, nothing to pay for.
    expect(tester.widget<WabButton>(find.byKey(_key('audition-filmmaker'))).onPressed, isNull);
    await _tap(tester, find.byKey(_key('choose-filmmaker-bright_woman')));
    await _tap(tester, find.byKey(_key('choose-filmmaker-calm_woman')));
    // Two voices saying the 20-character default line.
    expect(find.text(r'Audition 2 voices · ≈ $0.004'), findsOneWidget);
    await tester.enterText(find.byKey(_key('audition-line-filmmaker')), 'Cut.');
    await tester.pump();
    expect(find.text(r'Audition 2 voices · ≈ $0.0008'), findsOneWidget);
    await _tap(tester, find.byKey(_key('audition-filmmaker')));
    expect(calls.auditioned!.map((choice) => choice.voiceId), <String>['bright_woman', 'calm_woman']);

    await _tap(tester, find.byKey(_key('play-voice.filmmaker.0001')));
    await _tap(tester, find.byKey(_key('lock-voice-filmmaker')));
    await _tap(tester, find.byKey(_key('load-voices')));
    await _tap(tester, find.byKey(_key('recommend-voices')));
    expect(calls.log, <String>[
      'audition filmmaker Cut.',
      'play /film/characters/filmmaker/voices/voice.filmmaker.0001.mp3',
      'lock filmmaker',
      'load',
      'recommend',
    ]);
    // A locked voice is not auditioned again.
    expect(find.byKey(_key('audition-interviewer')), findsNothing);
  });

  testWidgets('a line is dubbed with its price shown, its take chosen and locked', (tester) async {
    final calls = await _pump(tester);
    expect(find.text('The Interviewer · line 1'), findsOneWidget);
    expect(find.text('(quietly) “Was it worth it?”'), findsOneWidget);
    expect(find.text('1.3 s · Barely said.'), findsOneWidget);
    expect(find.text(r'Dub 1 take · ≈ $0.0016'), findsOneWidget);

    await tester.enterText(find.byKey(_key('dub-note-office_night-1')), 'Softer.');
    await _tap(tester, find.byKey(_key('dub-takes-office_night-1')));
    await tester.tap(find.text('3 takes').last);
    await tester.pumpAndSettle();
    expect(find.text(r'Dub 3 takes · ≈ $0.0048'), findsOneWidget);
    await _tap(tester, find.byKey(_key('dub-office_night-1')));
    expect((calls.dubbed!.takes, calls.dubbed!.note, calls.dubbed!.delivery), (3, 'Softer.', null));

    await _tap(tester, find.byKey(_key('select-line.office_night.0001')));
    await _tap(tester, find.byKey(_key('reject-line.office_night.0001')));
    expect(calls.log, <String>[
      'dub office_night 1',
      'select take line.office_night.0001',
      'reject take line.office_night.0001',
    ]);
  });

  testWidgets('a line waits for its speaker\'s voice; nothing is asked while working', (tester) async {
    const unvoiced = KinetoVoices(
      characterCostMicros: 0,
      voices: _catalog,
      characters: <KinetoCharacterVoice>[_filmmaker],
      lines: <KinetoLine>[
        KinetoLine(sceneId: 'office_night', number: 2, text: 'It was.', characterId: 'filmmaker'),
      ],
    );
    await _pump(tester, voices: unvoiced);
    expect(find.text("Lock The Filmmaker's voice first; every take is spoken in it."), findsOneWidget);
    expect(find.byKey(_key('dub-office_night-2')), findsNothing);
    expect(find.text('Speech is free with this provider.'), findsOneWidget);
  });
}
