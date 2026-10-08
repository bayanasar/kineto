import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/native/kineto_engine.dart';

Matcher _refusedWith(int code) =>
    throwsA(isA<KinetoProjectException>().having((error) => error.code, 'code', code));

/// Lock shot [shot]'s demo keyframe and make [count] free demo takes of its
/// video, waiting for them to arrive.
Future<void> _videos(KinetoProjectSession session, int shot, {int count = 1, String? note, String? branch}) async {
  final arrived = session.videoEvents.take(count).toList();
  await session.generateShotVideos(shot, count: count, note: note, branchFrom: branch);
  final events = await arrived.timeout(const Duration(seconds: 10));
  expect(events.map((event) => event.errorCode), List<int>.filled(count, 0));
}

void main() {
  test('takes, scenes, stored media and the package are read and decided through the native session', () async {
    final sandbox = Directory.systemTemp.createTempSync('kineto-handoff-ffi-');
    try {
      final target = Directory('${sandbox.path}${Platform.pathSeparator}film');
      final session = await KinetoProjectSession.createTextProject(
        path: target.path,
        projectId: 'handoff',
        title: 'Handoff',
        createdAt: '2026-10-03T00:00:00Z',
        language: 'en',
        sourceText: 'A filmmaker waits across the table.\n',
      );
      for (final shot in <int>[0, 1]) {
        await session.generateShotCandidates(shot);
        await session.selectShotCandidate(shot, 0);
        await session.lockShotSelection(shot);
      }
      var scenes = session.sceneProgress;
      expect(scenes.single.sceneId, 'scene_001');
      expect(scenes.single.complete, isFalse);
      expect(
        scenes.single.shots.map((shot) => (shot.shotId, shot.keyframe, shot.video)),
        <(String, KinetoStage, KinetoStage)>[
          ('shot_001', KinetoStage.locked, KinetoStage.missing),
          ('shot_002', KinetoStage.locked, KinetoStage.missing),
        ],
      );

      // The demo makes 2-second clips, free, whatever the plan asks.
      final estimate = session.videoEstimate(0)!;
      expect((estimate.clipSeconds, estimate.costMicros), (2, 0));
      expect(estimate.costFor(3), 'free');

      // Two takes, then one refined by a note, then a branch from it.
      await _videos(session, 0, count: 2);
      await _videos(session, 0, note: 'Slower.');
      await _videos(session, 0, note: 'Hold on his eyes', branch: 'shot_001_v0002_candidate_a');
      await expectLater(session.generateShotVideos(0, count: 5), _refusedWith(KinetoProjectException.invalidArgument));
      var takes = session.videoTakes(0);
      expect(takes.map((take) => take.id), <String>[
        'shot_001_v0001_candidate_a',
        'shot_001_v0001_candidate_b',
        'shot_001_v0002_candidate_a',
        'shot_001_v0003_candidate_a',
      ]);
      expect(takes.map((take) => take.letter), <String>['a', 'b', 'a', 'a']);
      expect(takes.where((take) => take.current).map((take) => take.generation), <int>[3]);
      final branch = takes.last;
      expect(branch.notes, <String>['Slower.', 'Hold on his eyes']);
      expect(branch.branchedFrom, 'shot_001_v0002_candidate_a');
      expect(branch.provider, 'local-demo-video');
      expect(branch.stale, isEmpty);
      expect(File(branch.mediaPath!).readAsBytesSync().sublist(4, 8), utf8.encode('ftyp'));
      await session.lockShotVideo(0);

      // Of two takes of the second shot, the director chooses the second.
      await _videos(session, 1, count: 2);
      await expectLater(session.lockShotVideo(1), _refusedWith(113));
      await session.selectShotVideo(1, 1);
      await session.lockShotVideo(1);
      expect(session.videoTakes(1).map((take) => take.status), <KinetoTakeStatus>[
        KinetoTakeStatus.candidate,
        KinetoTakeStatus.locked,
      ]);
      scenes = session.sceneProgress;
      expect(scenes.single.complete, isTrue);

      // Stored media: the demo clips are one file, and a dry run comes before
      // anything is let go.
      var storage = session.storage;
      expect(storage.recentTakes, 6);
      expect(storage.usageBytes, greaterThan(0));
      expect(storage.owners.map((owner) => owner.$1), <String>['shot scene_001/shot_001', 'shot scene_001/shot_002']);
      expect(storage.plan, isNull);
      await expectLater(session.collectMedia(), _refusedWith(KinetoProjectException.noCollectionPlan));
      await session.planCollection();
      expect(session.storage.plan!.items, isEmpty, reason: 'every take is among the latest six');
      await session.verifyMedia();
      expect(session.storage.mismatched, isEmpty);
      expect(session.storage.missing, isEmpty);

      // The package: every shot locked, so it is complete.
      final out = Directory('${sandbox.path}${Platform.pathSeparator}out')..createSync();
      var export = session.exportState;
      expect(export.complete, isTrue);
      expect(export.ready, 2);
      expect(export.last, isNull);
      await session.exportPackage(destination: out.path, name: 'dailies');
      export = session.exportState;
      expect(export.last!.complete, isTrue);
      expect(export.last!.shots, 2);
      expect(export.last!.path, '${out.path}${Platform.pathSeparator}dailies');
      final manifest = jsonDecode(File('${export.last!.path}${Platform.pathSeparator}manifest.json').readAsStringSync())
          as Map<String, Object?>;
      expect(manifest['complete'], isTrue);
      expect(manifest['total_runtime_seconds'], 4);
      expect((manifest['timeline'] as Map<String, Object?>)['otio'], 'timeline.otio');
      expect(File('${export.last!.path}${Platform.pathSeparator}timeline.edl').existsSync(), isTrue);
      await expectLater(
        session.exportPackage(destination: out.path, name: 'dailies'),
        _refusedWith(KinetoProjectException.exportExists),
      );
      // Without the timeline, at the director's choice.
      await session.exportPackage(destination: out.path, name: 'dailies', replace: true, timeline: false);
      final bare = jsonDecode(File('${export.last!.path}${Platform.pathSeparator}manifest.json').readAsStringSync())
          as Map<String, Object?>;
      expect(bare.containsKey('timeline'), isFalse);
      expect(File('${export.last!.path}${Platform.pathSeparator}timeline.otio').existsSync(), isFalse);
      session.close();

      // Every take and choice is in the project's files.
      Directory('${target.path}${Platform.pathSeparator}.kineto').deleteSync(recursive: true);
      final reopened = await KinetoProjectSession.open(target.path);
      takes = reopened.videoTakes(0);
      expect(takes, hasLength(4));
      expect(takes.last.status, KinetoTakeStatus.locked);
      expect(reopened.sceneProgress.single.complete, isTrue);
      expect(reopened.exportState.complete, isTrue);
      expect(reopened.exportState.last, isNull, reason: 'the last package is the session\'s, not the project\'s');
      reopened.close();
    } finally {
      sandbox.deleteSync(recursive: true);
    }
  });

  test('a shot not ready keeps the package from being written unless asked', () async {
    final sandbox = Directory.systemTemp.createTempSync('kineto-handoff-ffi-');
    try {
      final session = await KinetoProjectSession.createTextProject(
        path: '${sandbox.path}${Platform.pathSeparator}film',
        projectId: 'partial',
        title: 'Partial',
        createdAt: '2026-10-03T00:00:00Z',
        language: 'en',
        sourceText: 'A filmmaker waits.\n',
      );
      await expectLater(
        session.exportPackage(destination: sandbox.path, name: 'nothing', partial: true),
        _refusedWith(KinetoProjectException.nothingToExport),
      );
      await session.generateShotCandidates(0);
      await session.selectShotCandidate(0, 0);
      await session.lockShotSelection(0);
      await _videos(session, 0);
      await session.lockShotVideo(0);
      final state = session.exportState;
      expect(state.ready, 1);
      expect(
        state.missing.map((missing) => (missing.shotId, missing.reason)),
        <(String, String)>[('shot_002', 'its keyframe is not locked')],
      );
      await expectLater(
        session.exportPackage(destination: sandbox.path, name: 'dailies'),
        _refusedWith(KinetoProjectException.exportIncomplete),
      );
      expect(Directory('${sandbox.path}${Platform.pathSeparator}dailies').existsSync(), isFalse);
      await session.exportPackage(destination: sandbox.path, name: 'dailies', partial: true);
      expect(session.exportState.last!.complete, isFalse);
      session.close();
    } finally {
      sandbox.deleteSync(recursive: true);
    }
  });
}
