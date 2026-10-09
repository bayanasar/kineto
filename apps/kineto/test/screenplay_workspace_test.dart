import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/native/kineto_engine.dart';
import 'package:kineto/screenplay_workspace.dart';
import 'package:wabisabi/wabisabi.dart';

const KinetoScreenplay _screenplay = KinetoScreenplay(
  revision: 2,
  title: 'The Last Line',
  cast: <KinetoCastMember>[
    KinetoCastMember(id: 'filmmaker', name: 'Daniel'),
    KinetoCastMember(id: 'interviewer', name: 'Mara'),
  ],
  scenes: <KinetoScene>[
    KinetoScene(
      id: 'office_night',
      heading: 'INT. SMALL OFFICE - NIGHT',
      setting: KinetoSceneSetting.interior,
      location: 'small_office',
      time: 'night',
      characters: <String>['filmmaker', 'interviewer'],
      dramaticGoal: 'The last question lands.',
      blocks: <KinetoScreenplayBlock>[
        KinetoScreenplayBlock(kind: KinetoBlockKind.action, text: 'Rain on the window.'),
        KinetoScreenplayBlock(
          kind: KinetoBlockKind.dialogue,
          character: 'interviewer',
          parenthetical: 'quietly',
          text: 'Was it worth it?',
        ),
      ],
    ),
  ],
);

const KinetoDocuments _written = KinetoDocuments(
  sourceRevision: 1,
  analysis: KinetoStoryAnalysis(
    revision: 1,
    logline: 'A filmmaker leaves before he can answer.',
    characterNames: <String>['Daniel', 'Mara'],
  ),
  screenplay: _screenplay,
  revisions: <KinetoRevision>[
    KinetoRevision(
      number: 1,
      status: KinetoRevisionStatus.superseded,
      origin: KinetoRevisionOrigin.generated,
      provider: 'kimi-code',
      model: 'agent-default',
    ),
    KinetoRevision(
      number: 2,
      status: KinetoRevisionStatus.candidate,
      origin: KinetoRevisionOrigin.edited,
      basedOn: 1,
    ),
  ],
);

const KinetoLanguageModel _kimiCode =
    KinetoLanguageModel(kind: KinetoLanguageModelKind.kimiCode, ready: true, model: 'agent-default');

final class _Calls {
  final List<String> log = <String>[];
  SceneDraft? saved;
}

Future<_Calls> _pump(
  WidgetTester tester, {
  KinetoDocuments documents = _written,
  KinetoLanguageModel model = _kimiCode,
  bool working = false,
  String? editing,
  String? error,
  bool offerResubmit = false,
}) async {
  tester.view.physicalSize = const Size(1280, 2400);
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  final calls = _Calls();
  await tester.pumpWidget(
    MaterialApp(
      theme: WabTheme.materialTheme(lightTheme: false),
      home: ScreenplayWorkspaceView(
        documents: documents,
        model: model,
        working: working,
        workingLabel: 'Writing the screenplay with Kimi Code',
        editingSceneId: editing,
        error: error,
        onResubmit: offerResubmit ? () => calls.log.add('resubmit') : null,
        onImport: (text) => calls.log.add('import $text'),
        onAnalyze: () => calls.log.add('analyse'),
        onWrite: () => calls.log.add('write'),
        onRewrite: (scene, direction) => calls.log.add('rewrite $scene $direction'),
        onEdit: (scene) => calls.log.add('edit $scene'),
        onSave: (draft) => calls.saved = draft,
        onLock: (revision) => calls.log.add('lock $revision'),
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

void main() {
  testWidgets('without a model nothing can be asked for, and the director is told why', (tester) async {
    final calls = await _pump(
      tester,
      documents: const KinetoDocuments(sourceRevision: 1),
      model: KinetoLanguageModel.none,
    );
    expect(find.text('Not analysed yet. The analysis finds the characters, locations and scenes the screenplay is written from.'), findsOneWidget);
    expect(find.textContaining('Choose one under Settings, Providers'), findsOneWidget);
    expect(find.text('No model · no model configured'), findsNWidgets(2));
    await _tap(tester, find.byKey(const ValueKey<String>('analyse')));
    await _tap(tester, find.byKey(const ValueKey<String>('write')));
    expect(calls.log, isEmpty);
  });

  testWidgets('an agent says its cost is unknown and the screenplay reads as a screenplay', (tester) async {
    final calls = await _pump(tester);
    expect(find.text('Kimi Code · cost unknown: covered by your Kimi Code sign-in'), findsNWidgets(2));
    expect(find.text('Screenplay · revision 2'), findsOneWidget);
    expect(find.text('Draft'), findsOneWidget);
    expect(find.text('1. INT. SMALL OFFICE - NIGHT'), findsOneWidget);
    expect(find.text('MARA'), findsOneWidget, reason: 'dialogue is attributed by cast name');
    expect(find.text('(quietly)'), findsOneWidget);
    expect(find.text('Revision 2 · edited from revision 1 · draft'), findsOneWidget);
    expect(find.text('Revision 1 · written by kimi-code'), findsOneWidget);

    await _tap(tester, find.byKey(const ValueKey<String>('lock')));
    await _tap(tester, find.byKey(const ValueKey<String>('edit-office_night')));
    await _tap(tester, find.byKey(const ValueKey<String>('write')));
    expect(calls.log, <String>['lock 2', 'edit office_night', 'write']);
  });

  testWidgets('a scene is edited in place and saved as a draft of whole blocks', (tester) async {
    final calls = await _pump(tester, editing: 'office_night');
    expect(find.text('Editing'), findsOneWidget);
    await tester.enterText(find.byKey(const ValueKey<String>('block-office_night-1-text')), 'Was any of it worth it?');
    await tester.enterText(find.byKey(const ValueKey<String>('block-office_night-0-text')), '   ');
    await tester.enterText(find.byKey(const ValueKey<String>('scene-office_night-time')), 'later');
    await _tap(tester, find.byKey(const ValueKey<String>('save-office_night')));

    final draft = calls.saved!;
    expect(draft.sceneId, 'office_night');
    expect(draft.time, 'later');
    expect(draft.blocks, const <KinetoScreenplayBlock>[
      KinetoScreenplayBlock(
        kind: KinetoBlockKind.dialogue,
        character: 'interviewer',
        parenthetical: 'quietly',
        text: 'Was any of it worth it?',
      ),
    ], reason: 'an emptied block is removed');
    // While editing, nothing else may change the screenplay.
    await _tap(tester, find.byKey(const ValueKey<String>('lock')));
    expect(calls.log, isEmpty);
  });

  testWidgets('a line of dialogue without a speaker is not saved', (tester) async {
    final calls = await _pump(tester, editing: 'office_night');
    await _tap(tester, find.text('Add dialogue'));
    await tester.enterText(find.byKey(const ValueKey<String>('block-office_night-2-text')), 'Who said that?');
    await _tap(tester, find.byKey(const ValueKey<String>('save-office_night')));
    expect(calls.saved, isNull);
    expect(find.text('Choose who speaks every line of dialogue.'), findsOneWidget);
  });

  testWidgets('while a document is being written every change waits', (tester) async {
    final calls = await _pump(tester, working: true);
    expect(find.text('Writing the screenplay with Kimi Code'), findsOneWidget);
    for (final key in <String>['analyse', 'write', 'lock', 'edit-office_night', 'rewrite-office_night']) {
      await _tap(tester, find.byKey(ValueKey<String>(key)));
    }
    expect(calls.log, isEmpty);
  });

  testWidgets('asking again at the risk of paying twice is offered only after that refusal', (tester) async {
    var calls = await _pump(tester, error: 'The model failed.');
    expect(find.text('The model failed.'), findsOneWidget);
    expect(find.byKey(const ValueKey<String>('resubmit-document')), findsNothing);

    calls = await _pump(tester, error: 'A paid call may already have been accepted.', offerResubmit: true);
    expect(find.textContaining('without the answer reaching Kineto'), findsOneWidget);
    expect(find.text('Ask again · may pay twice'), findsOneWidget);
    await _tap(tester, find.byKey(const ValueKey<String>('resubmit-document')));
    expect(calls.log, <String>['resubmit']);
  });

  testWidgets('a locked revision says so and offers no lock', (tester) async {
    await _pump(
      tester,
      documents: const KinetoDocuments(
        sourceRevision: 1,
        analysis: KinetoStoryAnalysis(revision: 1, logline: 'x', characterNames: <String>['Mara']),
        screenplay: _screenplay,
        lockedRevision: 2,
      ),
    );
    expect(find.text('Locked'), findsOneWidget);
    expect(find.byKey(const ValueKey<String>('lock')), findsNothing);
  });

  testWidgets('imported text reaches the project once the dialog has closed', (tester) async {
    final calls = await _pump(tester);
    await _tap(tester, find.text('Import text'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byKey(const ValueKey<String>('import-text')), 'A new draft of the story.');
    await _tap(tester, find.byKey(const ValueKey<String>('import-confirm')));
    // The dialog animates out with its text field still built.
    await tester.pumpAndSettle();
    expect(find.text('Import story text'), findsNothing);
    expect(calls.log, <String>['import A new draft of the story.']);
  });

  testWidgets('a rewrite direction reaches the project once the dialog has closed', (tester) async {
    final calls = await _pump(tester);
    await _tap(tester, find.byKey(const ValueKey<String>('rewrite-office_night')));
    await tester.pumpAndSettle();
    await tester.enterText(find.byKey(const ValueKey<String>('rewrite-direction')), '  Make it colder.  ');
    await _tap(tester, find.byKey(const ValueKey<String>('rewrite-confirm')));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey<String>('rewrite-direction')), findsNothing);
    expect(calls.log, <String>['rewrite office_night Make it colder.']);
  });
}
