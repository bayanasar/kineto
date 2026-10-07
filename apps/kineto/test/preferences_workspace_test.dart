import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/native/kineto_engine.dart';
import 'package:kineto/preferences_workspace.dart';
import 'package:wabisabi/wabisabi.dart';

const KinetoLanguageModel _kimiCode =
    KinetoLanguageModel(kind: KinetoLanguageModelKind.kimiCode, ready: true, model: 'agent-default');

const KinetoVoices _profile = KinetoVoices(
  preferences: <KinetoPreference>[
    KinetoPreference(
      dimension: KinetoDimension.lighting,
      stance: KinetoStance.prefer,
      value: 'low-key',
      strengthPermille: 800,
      keptFrom: 1,
    ),
    KinetoPreference(
      dimension: KinetoDimension.cameraMotion,
      stance: KinetoStance.avoid,
      value: 'handheld',
      keptFrom: 2,
      reason: 'The chosen takes all hold still.',
      evidence: <String>['office_night_02_g0001_candidate_b'],
    ),
  ],
  preferencesRevision: 2,
  preferenceRevisions: <KinetoPreferenceRevision>[
    KinetoPreferenceRevision(number: 1, origin: KinetoRevisionOrigin.edited),
    KinetoPreferenceRevision(number: 2, origin: KinetoRevisionOrigin.edited),
  ],
  suggestions: <KinetoSuggestion>[
    KinetoSuggestion(
      batch: 1,
      index: 0,
      dimension: KinetoDimension.framing,
      stance: KinetoStance.prefer,
      value: 'tight singles',
      reason: 'Every chosen keyframe frames one face.',
      evidence: <String>['office_night_02_g0001_candidate_a', 'office_night_02_g0002_candidate_c'],
    ),
  ],
  unconsideredChoices: 6,
  suggestionsReady: true,
  planPreferencesChanged: true,
);

final class _Calls {
  final List<String> log = <String>[];
  List<KinetoPreference>? saved;
}

Future<_Calls> _pump(WidgetTester tester, {KinetoVoices voices = _profile}) async {
  tester.view.physicalSize = const Size(1400, 2000);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  final calls = _Calls();
  await tester.pumpWidget(
    MaterialApp(
      theme: WabTheme.materialTheme(),
      home: PreferencesWorkspaceView(
        voices: voices,
        model: _kimiCode,
        onSave: (entries) => calls.saved = entries,
        onSuggest: () => calls.log.add('suggest'),
        onAccept: (suggestion, value) => calls.log.add('accept ${suggestion.batch}/${suggestion.index} $value'),
        onReject: (suggestion) => calls.log.add('reject ${suggestion.batch}/${suggestion.index}'),
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
  testWidgets('the profile is edited entry by entry and saved whole, keeping where each came from', (tester) async {
    final calls = await _pump(tester);
    expect(find.text('Your preferences · revision 2'), findsOneWidget);
    expect(find.text('The shot plan was made with others'), findsOneWidget);
    expect(find.text('From a suggestion: The chosen takes all hold still.'), findsOneWidget);

    await tester.enterText(find.byKey(_key('preference-0-value')), 'low-key, one source');
    await _tap(tester, find.byKey(_key('add-preference')));
    await tester.enterText(find.byKey(_key('preference-2-value')), 'slow dolly');
    await _tap(tester, find.byKey(_key('save-preferences')));
    final saved = calls.saved!;
    expect(saved.map((entry) => (entry.value, entry.keptFrom, entry.strengthPermille)), <(String, int, int?)>[
      ('low-key, one source', 1, 800),
      ('handheld', 2, null),
      ('slow dolly', 0, null),
    ]);
    expect(saved[1].fromSuggestion, isTrue);

    // An entry without words is not saved.
    calls.saved = null;
    await _tap(tester, find.byKey(_key('add-preference')));
    await _tap(tester, find.byKey(_key('save-preferences')));
    expect(calls.saved, isNull);
    expect(find.text('Every preference needs words: what to prefer or avoid.'), findsOneWidget);
    await _tap(tester, find.byKey(_key('preference-3-remove')));
    await _tap(tester, find.byKey(_key('preference-0-remove')));
    await _tap(tester, find.byKey(_key('save-preferences')));
    expect(calls.saved!.map((entry) => entry.value), <String>['handheld', 'slow dolly']);
  });

  testWidgets('a suggestion is accepted as suggested or in the director\'s words, or rejected', (tester) async {
    final calls = await _pump(tester);
    expect(find.text('Worth asking'), findsOneWidget);
    expect(find.text('6 new choices since the last suggestions; 5 make it worth asking.'), findsOneWidget);
    expect(find.text('Prefer · framing · tight singles'), findsOneWidget);
    expect(
      find.text(
        'Seen in office_night_02_g0001_candidate_a, office_night_02_g0002_candidate_c '
        '(compared by their prompts and notes, not the images)',
      ),
      findsOneWidget,
    );
    await _tap(tester, find.byKey(_key('suggest-preferences')));
    await _tap(tester, find.byKey(_key('suggestion-1-0-accept')));
    await tester.enterText(find.byKey(_key('suggestion-1-0-words')), 'close singles');
    await tester.pump();
    await _tap(tester, find.byKey(_key('suggestion-1-0-accept')));
    await _tap(tester, find.byKey(_key('suggestion-1-0-reject')));
    expect(calls.log, <String>['suggest', 'accept 1/0 null', 'accept 1/0 close singles', 'reject 1/0']);
  });

  testWidgets('with no new choices there is nothing to ask', (tester) async {
    await _pump(tester, voices: const KinetoVoices());
    expect(find.text('Your preferences'), findsOneWidget);
    expect(tester.widget<WabButton>(find.byKey(_key('suggest-preferences'))).onPressed, isNull);
  });
}
