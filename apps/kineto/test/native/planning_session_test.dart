import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/native/kineto_engine.dart';

/// A copy of `fixtures/projects/planned`: the production fixture with its
/// bible locked, the sample shot plan locked so its two shots are in play,
/// and the second shot generated twice, the second time refined by a note.
Directory _fixtureCopy(Directory sandbox) {
  final source = Directory(
    ['..', '..', 'fixtures', 'projects', 'planned'].join(Platform.pathSeparator),
  );
  final target = Directory([sandbox.path, 'film'].join(Platform.pathSeparator));
  for (final entity in source.listSync(recursive: true)) {
    final relative = entity.path.substring(source.path.length + 1);
    final destination = [target.path, relative].join(Platform.pathSeparator);
    if (entity is Directory) {
      Directory(destination).createSync(recursive: true);
    } else if (entity is File) {
      File(destination).parent.createSync(recursive: true);
      entity.copySync(destination);
    }
  }
  return target;
}

Matcher _refusedWith(int code) =>
    throwsA(isA<KinetoProjectException>().having((error) => error.code, 'code', code));

void main() {
  test('a shot plan, its steps and its takes are read and decided through the native session', () async {
    final sandbox = Directory.systemTemp.createTempSync('kineto-planning-ffi-');
    try {
      final target = _fixtureCopy(sandbox);
      final session = await KinetoProjectSession.open(target.path);

      var planning = session.planning;
      expect(planning.revision, 1);
      expect(planning.planLocked, isTrue);
      expect(planning.stale, isEmpty);
      expect(planning.revisions.single.provider, 'fake-llm');
      final scene = planning.scenes.single;
      expect(scene.sceneId, 'office_night');
      expect(scene.shots.map((shot) => shot.id), <String>['office_night_01', 'office_night_02']);
      final second = scene.shots[1];
      expect(second.direction, KinetoShotDirection.reaction);
      expect(second.framing, KinetoFraming.mediumCloseUp);
      expect(second.cameraMotion, KinetoCameraMotion.slowDollyIn);
      expect(second.lens, '50mm');
      expect(second.durationTarget, 5);
      expect(second.dialogue, 'Was it worth it?');
      expect(second.characters, <String>['filmmaker']);
      expect(second.continuityFrom, 'office_night_01');
      expect(scene.shots.first.continuity, <String>['Rain on the window behind them.']);

      final steps = <String, KinetoWorkflowStep>{for (final step in planning.steps) step.id: step};
      expect(steps.keys.first, 'source');
      expect(steps['shot_plan']!.state, KinetoStepState.done);
      expect(steps['casting']!.state, KinetoStepState.awaitingApproval, reason: 'the interviewer is not cast');
      expect(steps['keyframes']!.state, KinetoStepState.awaitingApproval);
      expect(steps['keyframes']!.waitingOn, <String>['casting']);
      expect(planning.workflowProblem, isNull);

      // The plan's shots are in play.
      expect(session.shotCount, 2);
      expect(session.shotId(1), 'office_night_02');
      expect(session.shotSceneId(1), 'office_night');
      final takes = session.takes(1);
      expect(takes.map((take) => take.generation), <int>[1, 1, 1, 2, 2, 2]);
      expect(takes.first.status, KinetoTakeStatus.superseded);
      expect(takes.last.status, KinetoTakeStatus.candidate);
      expect(takes.map((take) => take.refined), <bool>[false, false, false, true, true, true]);
      expect(takes.last.provider, 'minimax');
      expect(File(takes.first.mediaPath!).existsSync(), isTrue);
      expect(session.staleReasons(1), <List<String>>[<String>[], <String>[], <String>[]]);
      expect(session.takes(0), isEmpty);

      // No language model is named for this test process.
      expect(session.documentCostMicros(KinetoDocumentKind.shotPlan), isNull);
      await expectLater(session.planShots(), _refusedWith(KinetoProjectException.modelNotConfigured));
      await expectLater(
        session.replanScene('office_night', direction: 'One take.'),
        _refusedWith(KinetoProjectException.modelNotConfigured),
      );

      // The director's edit of the scene, then a new lock: the shot's takes
      // say what changed.
      await session.saveScenePlan(
        1,
        KinetoScenePlan(
          sceneId: 'office_night',
          shots: <KinetoPlannedShot>[
            scene.shots.first,
            second.copyWith(emotion: 'Nothing shows yet.'),
          ],
        ),
      );
      await expectLater(
        session.saveScenePlan(1, scene),
        _refusedWith(KinetoProjectException.notLatest),
      );
      await expectLater(
        session.saveScenePlan(
          2,
          KinetoScenePlan(sceneId: 'office_night', shots: <KinetoPlannedShot>[
            scene.shots.first,
            second.copyWith(durationTarget: 45),
          ]),
        ),
        _refusedWith(KinetoProjectException.invalidDocument),
      );
      await session.lockShotPlan(2);
      planning = session.planning;
      expect(planning.lockedRevision, 2);
      expect(planning.shot('office_night_02')!.emotion, 'Nothing shows yet.');
      expect(session.shotSnapshot(1).stale, isTrue);
      expect(
        session.staleReasons(1),
        List<List<String>>.filled(3, <String>['intent.emotion']),
      );

      // The local demo refines without paying; the note is the director's.
      await session.refineShot(0, 'Wider, show the window.');
      expect(session.takes(0), hasLength(3));
      await expectLater(session.refineShot(0, ' '), _refusedWith(KinetoProjectException.invalidArgument));

      // The reaction shot goes first and a new shot closes the scene: the
      // shots in play follow, each keeping its own takes.
      final edited = session.planning.scenes.single.shots;
      await session.saveScenePlan(
        2,
        KinetoScenePlan(
          sceneId: 'office_night',
          shots: <KinetoPlannedShot>[
            KinetoPlannedShot(
              id: edited[1].id,
              purpose: edited[1].purpose,
              direction: edited[1].direction,
              framing: edited[1].framing,
              lens: edited[1].lens,
              cameraMotion: edited[1].cameraMotion,
              action: edited[1].action,
              dialogue: edited[1].dialogue,
              emotion: edited[1].emotion,
              durationTarget: edited[1].durationTarget,
              characters: edited[1].characters,
            ),
            edited[0],
            const KinetoPlannedShot(
              id: 'office_night_03',
              purpose: 'Show the empty chair.',
              direction: KinetoShotDirection.spatialClarity,
              framing: KinetoFraming.wide,
              lens: '28mm',
              cameraMotion: KinetoCameraMotion.still,
              action: 'The chair is empty.',
              emotion: 'Absence.',
              durationTarget: 3,
              continuityFrom: 'office_night_01',
            ),
          ],
        ),
      );
      await session.lockShotPlan(3);
      expect(session.shotCount, 3);
      expect(<String>[for (var index = 0; index < 3; index++) session.shotId(index)],
          <String>['office_night_02', 'office_night_01', 'office_night_03']);
      expect(session.takes(0), hasLength(6));
      expect(session.takes(1), hasLength(3));
      expect(session.takes(2), isEmpty);
      session.close();

      // Every decision is in the project's files.
      Directory([target.path, '.kineto'].join(Platform.pathSeparator)).deleteSync(recursive: true);
      final reopened = await KinetoProjectSession.open(target.path);
      expect(reopened.planning.lockedRevision, 3);
      expect(reopened.shotId(0), 'office_night_02');
      expect(reopened.shotId(2), 'office_night_03');
      expect(reopened.takes(0), hasLength(6));
      expect(reopened.staleReasons(0).first, contains('intent.emotion'));

      // In one edit the reaction shot goes and a new shot comes, numbered as
      // the removed one by the client. Sent as added, it is a new shot: it
      // gets a number no shot has had and none of the removed shot's takes.
      final current = reopened.planning.scenes.single.shots;
      await reopened.saveScenePlan(
        3,
        KinetoScenePlan(
          sceneId: 'office_night',
          shots: <KinetoPlannedShot>[
            current[1],
            current[2],
            const KinetoPlannedShot(
              id: 'office_night_02',
              purpose: 'Hold on the door.',
              direction: KinetoShotDirection.spatialClarity,
              framing: KinetoFraming.wide,
              lens: '28mm',
              cameraMotion: KinetoCameraMotion.still,
              action: 'The door stays shut.',
              emotion: 'Waiting.',
              durationTarget: 3,
              added: true,
            ),
          ],
        ),
      );
      expect(
        reopened.planning.scenes.single.shots.map((shot) => shot.id),
        <String>['office_night_01', 'office_night_03', 'office_night_04'],
      );
      await reopened.lockShotPlan(4);
      expect(reopened.shotId(2), 'office_night_04');
      expect(reopened.takes(2), isEmpty);
      final removed = File(
        [target.path, 'scenes', 'office_night', 'shots', 'office_night_02', 'artifacts.json']
            .join(Platform.pathSeparator),
      );
      expect(removed.readAsStringSync(), contains('office_night_02_g0002'));
      reopened.close();
    } finally {
      sandbox.deleteSync(recursive: true);
    }
  });
}
