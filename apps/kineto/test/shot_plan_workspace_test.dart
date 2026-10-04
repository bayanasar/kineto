import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/native/kineto_engine.dart';
import 'package:kineto/shot_plan_workspace.dart';
import 'package:wabisabi/wabisabi.dart';

const KinetoPlannedShot _wide = KinetoPlannedShot(
  id: 'office_night_01',
  purpose: 'Establish the two of them across the desk.',
  direction: KinetoShotDirection.spatialClarity,
  framing: KinetoFraming.wide,
  lens: '28mm',
  cameraMotion: KinetoCameraMotion.still,
  action: 'The interviewer closes her notebook.',
  emotion: 'A held breath.',
  durationTarget: 4,
  characters: <String>['filmmaker', 'interviewer'],
  continuity: <String>['Rain on the window.'],
);

const KinetoPlannedShot _close = KinetoPlannedShot(
  id: 'office_night_02',
  purpose: 'Let the last question land on him.',
  direction: KinetoShotDirection.reaction,
  framing: KinetoFraming.mediumCloseUp,
  lens: '50mm',
  cameraMotion: KinetoCameraMotion.slowDollyIn,
  action: 'He hears the question and does not answer.',
  dialogue: 'Was it worth it?',
  emotion: 'A quiet, withheld reaction.',
  durationTarget: 5,
  characters: <String>['filmmaker'],
  continuityFrom: 'office_night_01',
);

const List<KinetoWorkflowStep> _steps = <KinetoWorkflowStep>[
  KinetoWorkflowStep(id: 'screenplay', description: 'Lock the screenplay.', state: KinetoStepState.done, approval: true),
  KinetoWorkflowStep(
    id: 'casting',
    description: 'Cast every main character.',
    state: KinetoStepState.skipped,
    approval: true,
    skippedWhen: <String>['no_main_characters'],
  ),
  KinetoWorkflowStep(
    id: 'shot_plan',
    description: 'Plan and lock the shots.',
    state: KinetoStepState.awaitingApproval,
    approval: true,
  ),
  KinetoWorkflowStep(
    id: 'keyframes',
    description: 'Lock a keyframe per shot.',
    state: KinetoStepState.waiting,
    approval: true,
    waitingOn: <String>['shot_plan'],
  ),
];

const KinetoPlanning _drafted = KinetoPlanning(
  revision: 1,
  scenes: <KinetoScenePlan>[
    KinetoScenePlan(sceneId: 'office_night', shots: <KinetoPlannedShot>[_wide, _close]),
  ],
  revisions: <KinetoRevision>[
    KinetoRevision(
      number: 1,
      status: KinetoRevisionStatus.candidate,
      origin: KinetoRevisionOrigin.generated,
      provider: 'kimi-code',
      model: 'agent-default',
    ),
  ],
  steps: _steps,
);

const KinetoLanguageModel _kimiCode =
    KinetoLanguageModel(kind: KinetoLanguageModelKind.kimiCode, ready: true, model: 'agent-default');

final class _Calls {
  final List<String> log = <String>[];
  KinetoScenePlan? saved;
}

Future<_Calls> _pump(
  WidgetTester tester, {
  KinetoPlanning planning = _drafted,
  bool ready = true,
  bool working = false,
  String? editing,
}) async {
  tester.view.physicalSize = const Size(1280, 3000);
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  final calls = _Calls();
  await tester.pumpWidget(
    MaterialApp(
      theme: WabTheme.materialTheme(lightTheme: false),
      home: ShotPlanWorkspaceView(
        planning: planning,
        model: _kimiCode,
        ready: ready,
        working: working,
        editingScene: editing,
        onPlan: () => calls.log.add('plan'),
        onReplan: (scene, direction) => calls.log.add('replan $scene $direction'),
        onEdit: (scene) => calls.log.add('edit $scene'),
        onSave: (scene) => calls.saved = scene,
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

ValueKey<String> _key(String key) => ValueKey<String>(key);

void main() {
  testWidgets('the plan reads as shots, scene by scene, and its draft is locked', (tester) async {
    final calls = await _pump(tester);
    expect(find.text('Shot plan · revision 1'), findsOneWidget);
    expect(find.text('Draft'), findsOneWidget);
    expect(find.text('office_night_02 · Medium close-up · 50mm · Slow dolly in · 5.0 s'), findsOneWidget);
    expect(find.text('“Was it worth it?”'), findsOneWidget);
    expect(find.text('Continues from office_night_01'), findsOneWidget);
    expect(find.text('Keep: Rain on the window.'), findsOneWidget);
    expect(find.text('shot plan · Awaiting approval'), findsOneWidget);
    expect(find.text('keyframes · Waiting'), findsOneWidget);
    expect(find.text('casting · Skipped'), findsOneWidget);
    expect(
      tester.widget<Tooltip>(find.ancestor(of: find.byKey(_key('step-casting')), matching: find.byType(Tooltip))).message,
      'Cast every main character.\nSkipped: no main characters',
    );
    expect(find.text('Kimi Code · cost unknown: billed by the Kimi Code subscription'), findsOneWidget);

    await _tap(tester, find.byKey(_key('lock-plan')));
    await _tap(tester, find.byKey(_key('edit-scene-office_night')));
    await _tap(tester, find.byKey(_key('plan-shots')));
    await _tap(tester, find.byKey(_key('replan-office_night')));
    await tester.enterText(find.byKey(_key('replan-direction')), 'One take only.');
    await tester.tap(find.byKey(_key('replan-confirm')));
    await tester.pumpAndSettle();
    expect(calls.log, <String>['lock 1', 'edit office_night', 'plan', 'replan office_night One take only.']);
  });

  testWidgets('a shot too short for its dialogue says so, and the editor says how long it takes', (tester) async {
    const short = KinetoPlannedShot(
      id: 'office_night_02',
      purpose: 'Let the last question land on him.',
      direction: KinetoShotDirection.reaction,
      framing: KinetoFraming.mediumCloseUp,
      lens: '50mm',
      cameraMotion: KinetoCameraMotion.slowDollyIn,
      action: 'He hears the question and does not answer.',
      dialogue: 'Was it worth it?',
      emotion: 'A quiet, withheld reaction.',
      durationTarget: 1,
      dialogueMillis: 1400,
      tooShort: true,
    );
    const planning = KinetoPlanning(
      revision: 1,
      scenes: <KinetoScenePlan>[
        KinetoScenePlan(sceneId: 'office_night', shots: <KinetoPlannedShot>[_wide, short]),
      ],
      revisions: <KinetoRevision>[
        KinetoRevision(number: 1, status: KinetoRevisionStatus.candidate, origin: KinetoRevisionOrigin.generated),
      ],
      steps: _steps,
    );
    await _pump(tester, planning: planning);
    expect(find.text('“Was it worth it?” · 1.4 s to say'), findsOneWidget);
    expect(find.text('Too short for its dialogue: 1.4 s to say in 1.0 s'), findsOneWidget);
    expect(find.byKey(_key('too-short-office_night_01')), findsNothing);

    await _pump(tester, planning: planning, editing: 'office_night');
    expect(find.byKey(_key('shot-office_night_02-dialogue-length')), findsOneWidget);
    expect(find.byKey(_key('shot-office_night_01-dialogue-length')), findsNothing);
  });

  testWidgets('without a locked screenplay and bible nothing is planned', (tester) async {
    final calls = await _pump(tester, planning: const KinetoPlanning(steps: _steps), ready: false);
    expect(find.text('Lock the screenplay and the bible first; shots are planned from both.'), findsOneWidget);
    await _tap(tester, find.byKey(_key('plan-shots')));
    expect(calls.log, isEmpty);
  });

  testWidgets('a locked plan that the bible has moved under says so', (tester) async {
    await _pump(
      tester,
      planning: const KinetoPlanning(
        revision: 1,
        lockedRevision: 1,
        scenes: <KinetoScenePlan>[
          KinetoScenePlan(sceneId: 'office_night', shots: <KinetoPlannedShot>[_wide]),
        ],
        stale: <KinetoPlanStaleness>[
          KinetoPlanStaleness(KinetoPlanStaleCause.entry, 'bible_character.filmmaker'),
        ],
        steps: _steps,
      ),
    );
    expect(find.text('Locked · changed since'), findsOneWidget);
    expect(find.text('May no longer match: bible_character.filmmaker changed.'), findsOneWidget);
    expect(find.byKey(_key('lock-plan')), findsNothing);
  });

  testWidgets('a scene is edited shot by shot and saved whole', (tester) async {
    final calls = await _pump(tester, editing: 'office_night');
    expect(find.text('Editing office night'), findsOneWidget);
    await tester.enterText(find.byKey(_key('shot-office_night_02-emotion')), 'Nothing shows yet.');
    await tester.enterText(find.byKey(_key('shot-office_night_02-duration')), '6.5');
    await _tap(tester, find.byKey(_key('save-scene-plan-office_night')));
    final saved = calls.saved!;
    expect(saved.sceneId, 'office_night');
    expect(saved.shots.map((shot) => shot.id), <String>['office_night_01', 'office_night_02']);
    expect(saved.shots[1].emotion, 'Nothing shows yet.');
    expect(saved.shots[1].durationTarget, 6.5);
    expect(saved.shots[1].continuityFrom, 'office_night_01');
    expect(saved.shots[0].continuity, <String>['Rain on the window.'], reason: 'what the editor does not show is kept');

    calls.saved = null;
    await tester.enterText(find.byKey(_key('shot-office_night_01-duration')), 'soon');
    await _tap(tester, find.byKey(_key('save-scene-plan-office_night')));
    expect(calls.saved, isNull);
    expect(find.textContaining('a duration in seconds'), findsOneWidget);
  });

  testWidgets('a scene\'s shots are reordered, added and removed, and continue only from earlier ones', (tester) async {
    final calls = await _pump(tester, editing: 'office_night');
    // The reaction shot goes first: it can no longer continue from the wide.
    await _tap(tester, find.byKey(_key('shot-office_night_02-up')));
    expect(find.text('1. office_night_02'), findsOneWidget);
    await _tap(tester, find.byKey(_key('save-scene-plan-office_night')));
    var saved = calls.saved!;
    expect(saved.shots.map((shot) => shot.id), <String>['office_night_02', 'office_night_01']);
    expect(saved.shots[0].continuityFrom, isNull);

    // A new shot after the wide, staged like it, is written before saving.
    calls.saved = null;
    await _tap(tester, find.byKey(_key('shot-office_night_01-add-after')));
    expect(find.text('3. office_night_03'), findsOneWidget);
    await _tap(tester, find.byKey(_key('save-scene-plan-office_night')));
    expect(calls.saved, isNull);
    await tester.enterText(find.byKey(_key('shot-office_night_03-purpose')), 'Show the empty chair.');
    await tester.enterText(find.byKey(_key('shot-office_night_03-action')), 'The chair is empty.');
    await tester.enterText(find.byKey(_key('shot-office_night_03-emotion')), 'Absence.');
    await _tap(tester, find.descendant(of: find.byKey(_key('shot-office_night_03-from')), matching: find.byType(DropdownButtonFormField<String>)));
    await tester.pumpAndSettle();
    await tester.tap(find.text('office_night_01').last);
    await tester.pumpAndSettle();

    // The reaction shot goes.
    await _tap(tester, find.byKey(_key('shot-office_night_02-remove')));
    await _tap(tester, find.byKey(_key('save-scene-plan-office_night')));
    saved = calls.saved!;
    expect(saved.shots.map((shot) => shot.id), <String>['office_night_01', 'office_night_03']);
    final added = saved.shots[1];
    expect(added.purpose, 'Show the empty chair.');
    expect(added.framing, KinetoFraming.wide);
    expect(added.characters, <String>['filmmaker', 'interviewer']);
    expect(added.continuityFrom, 'office_night_01');
    expect(added.continuity, isEmpty);
    expect(added.added, isTrue);
    expect(saved.shots[0].added, isFalse);
  });

  testWidgets('a shot removed and another added in one edit never shares its number', (tester) async {
    final calls = await _pump(tester, editing: 'office_night');
    await _tap(tester, find.byKey(_key('shot-office_night_02-remove')));
    await _tap(tester, find.byKey(_key('shot-office_night_01-add-after')));
    // Not office_night_02, which the removed shot keeps with its takes.
    expect(find.text('2. office_night_03'), findsOneWidget);
    await tester.enterText(find.byKey(_key('shot-office_night_03-purpose')), 'Show the empty chair.');
    await tester.enterText(find.byKey(_key('shot-office_night_03-action')), 'The chair is empty.');
    await tester.enterText(find.byKey(_key('shot-office_night_03-emotion')), 'Absence.');
    await _tap(tester, find.byKey(_key('save-scene-plan-office_night')));
    final saved = calls.saved!;
    expect(saved.shots.map((shot) => (shot.id, shot.added)), <(String, bool)>[
      ('office_night_01', false),
      ('office_night_03', true),
    ]);
  });

  testWidgets('while something is planned every change waits', (tester) async {
    final calls = await _pump(tester, working: true);
    for (final key in <String>['plan-shots', 'lock-plan', 'edit-scene-office_night', 'replan-office_night']) {
      await _tap(tester, find.byKey(_key(key)));
    }
    expect(calls.log, isEmpty);
  });
}
