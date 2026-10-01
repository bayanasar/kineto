import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/native/kineto_engine.dart';

void main() {
  test('canonical project create and open round-trip through Code Asset', () async {
    final sandbox = Directory.systemTemp.createTempSync('kineto-project-ffi-');
    final target = Directory(_path(sandbox.path, 'film'));

    try {
      final created = await _createProject(target, 'ffi_project_001');

      expect(created.formatVersion, 1);
      expect(created.isReadOnly, isFalse);
      expect(created.projectId, 'ffi_project_001');
      expect(created.title, 'FFI Project');
      expect(File(_path(target.path, 'project.toml')).existsSync(), isTrue);
      expect(
        File(_path(target.path, 'source', 'story.txt')).readAsStringSync(),
        'A filmmaker waits across the table.\n',
      );
      // The bootstrap scene's intent is written with the project.
      final shot = jsonDecode(
        File(_path(target.path, 'scenes', 'scene_001', 'shots', 'shot_001', 'shot.json'))
            .readAsStringSync(),
      ) as Map<String, Object?>;
      expect(shot['action'], contains('filmmaker'));

      // No provider was named, so the free demo generator is used.
      final provider = created.imageProvider;
      expect(provider.kind, KinetoImageProviderKind.localDemo);
      expect(provider.ready, isTrue);
      expect(provider.isPaid, isFalse);
      expect(provider.generationCostMicros, 0);

      // A second writer session is refused while this one is open.
      await expectLater(
        KinetoProjectSession.open(target.path),
        throwsA(
          isA<KinetoProjectException>().having(
            (error) => error.code,
            'code',
            KinetoProjectException.busy,
          ),
        ),
      );

      created.close();
      expect(() => created.title, throwsStateError);
      expect(() => created.shotSnapshot(0), throwsStateError);
      created.close();

      final reopened = await KinetoProjectSession.open(target.path);
      expect(reopened.formatVersion, 1);
      expect(reopened.isReadOnly, isFalse);
      expect(reopened.projectId, 'ffi_project_001');
      expect(reopened.title, 'FFI Project');
      reopened.close();
    } finally {
      sandbox.deleteSync(recursive: true);
    }
  });

  test('queued commands resolve in submission order off the UI isolate', () async {
    final sandbox = Directory.systemTemp.createTempSync('kineto-project-ffi-');
    final target = Directory(_path(sandbox.path, 'film'));

    try {
      final project = await _createProject(target, 'ffi_project_queue');
      final order = <String>[];
      // Nothing is awaited between submissions: each call only queues.
      final futures = <Future<void>>[
        project.generateShotCandidates(0).then((_) => order.add('generate')),
        project.selectShotCandidate(0, 1).then((_) => order.add('select')),
        project.lockShotSelection(0).then((_) => order.add('lock')),
      ];
      expect(order, isEmpty);
      await Future.wait(futures);

      expect(order, <String>['generate', 'select', 'lock']);
      final snapshot = project.shotSnapshot(0);
      expect(snapshot.locked, isTrue);
      expect(snapshot.selectedIndex, 1);
      project.close();
    } finally {
      sandbox.deleteSync(recursive: true);
    }
  });

  test('closing a session fails its unanswered commands instead of hanging', () async {
    final sandbox = Directory.systemTemp.createTempSync('kineto-project-ffi-');
    final target = Directory(_path(sandbox.path, 'film'));

    try {
      final project = await _createProject(target, 'ffi_project_close');
      final pending = List<Future<void>>.generate(
        10,
        (_) => project.generateShotCandidates(0),
      );
      project.close();

      for (final future in pending) {
        await expectLater(
          future,
          throwsA(isA<KinetoProjectSessionClosedException>()),
        );
      }
      expect(() => project.generateShotCandidates(0), throwsStateError);

      // The writer lock was released by close.
      final reopened = await KinetoProjectSession.open(target.path);
      reopened.close();
    } finally {
      sandbox.deleteSync(recursive: true);
    }
  });

  test('reopening completes a generation that was never acknowledged', () async {
    final sandbox = Directory.systemTemp.createTempSync('kineto-project-ffi-');
    final target = Directory(_path(sandbox.path, 'film'));

    try {
      var project = await _createProject(target, 'ffi_project_recovery');
      expect(project.recoveries, isEmpty);
      await project.generateShotCandidates(0);
      project.close();

      // As if the app died after attaching the candidates but before the
      // write-ahead intents were acknowledged.
      final intents = Directory(_path(target.path, '.kineto', 'jobs', 'intents'))
          .listSync()
          .whereType<File>()
          .toList();
      expect(intents, hasLength(3));
      for (final intent in intents) {
        final json = jsonDecode(intent.readAsStringSync()) as Map<String, dynamic>;
        json['state'] = 'invoked';
        intent.writeAsStringSync(jsonEncode(json));
      }

      project = await KinetoProjectSession.open(target.path);
      final recoveries = project.recoveries;
      expect(recoveries, hasLength(1));
      expect(recoveries.single.completed, isTrue);
      expect(recoveries.single.shotIndex, 0);
      expect(recoveries.single.revisionOrCount, 1);
      expect(project.shotSnapshot(0).generation, 1);
      expect(project.shotSnapshot(0).candidateCount, 3);
      for (final intent in intents) {
        final json = jsonDecode(intent.readAsStringSync()) as Map<String, dynamic>;
        expect(json['state'], 'reconciled');
      }
      project.close();

      project = await KinetoProjectSession.open(target.path);
      expect(project.recoveries, isEmpty);
      project.close();
    } finally {
      sandbox.deleteSync(recursive: true);
    }
  });

  test('legacy shot hashes stay read-only until an explicit migration', () async {
    final sandbox = Directory.systemTemp.createTempSync('kineto-project-ffi-');
    final target = Directory(_path(sandbox.path, 'film'));

    try {
      var project = await _createProject(target, 'ffi_project_legacy');
      await project.generateShotCandidates(0);
      project.close();

      // Rewrite the records in the pre-sha256 form an older build produced.
      final shotDir = _path(
        target.path,
        'scenes',
        'scene_001',
        'shots',
        'shot_001',
      );
      final artifactsFile = File(_path(shotDir, 'artifacts.json'));
      final artifacts = (jsonDecode(
        artifactsFile.readAsStringSync(),
      ) as List<dynamic>).cast<Map<String, dynamic>>();
      for (final artifact in artifacts) {
        artifact['content_hash'] = 'demo:${artifact['artifact_id']}';
        artifact['input_hash'] = 'kineto:shot-intent:v1:shot_001:reaction';
      }
      final legacy = const JsonEncoder.withIndent('  ').convert(artifacts);
      artifactsFile.writeAsStringSync(legacy);

      project = await KinetoProjectSession.open(target.path);
      var snapshot = project.shotSnapshot(0);
      expect(snapshot.migrationPending, isTrue);
      expect(snapshot.hasInvalidHashes, isFalse);
      expect(snapshot.changesBlocked, isTrue);
      await expectLater(project.generateShotCandidates(0),
        throwsA(
          isA<KinetoProjectException>().having(
            (error) => error.code,
            'code',
            KinetoProjectException.migrationRequired,
          ),
        ),
      );
      expect(artifactsFile.readAsStringSync(), legacy);

      await project.migrateLegacyHashes(0);
      snapshot = project.shotSnapshot(0);
      expect(snapshot.migrationPending, isFalse);
      expect(snapshot.changesBlocked, isFalse);
      expect(
        File(_path(shotDir, 'artifacts.pre-sha256-migration.json'))
            .readAsStringSync(),
        legacy,
      );
      await project.generateShotCandidates(0);
      expect(project.shotSnapshot(0).generation, 2);
      project.close();
    } finally {
      sandbox.deleteSync(recursive: true);
    }
  });

  test('shot approval survives reopen and deletion of .kineto cache', () async {
    final sandbox = Directory.systemTemp.createTempSync('kineto-project-ffi-');
    final target = Directory(_path(sandbox.path, 'film'));

    try {
      var project = await _createProject(target, 'ffi_project_durable');
      await project.setShotDirection(0, KinetoShotDirection.tension);
      await project.generateShotCandidates(0);
      await project.selectShotCandidate(0, 2);
      await project.lockShotSelection(0);

      final before = project.shotSnapshot(0);
      final selectionFile = File(
        _path(
          target.path,
          'scenes',
          'scene_001',
          'shots',
          'shot_001',
          'selection.json',
        ),
      );
      final selectedBefore =
          (jsonDecode(selectionFile.readAsStringSync()) as Map<String, dynamic>)[
              'selected_artifact_id'
          ] as String;
      expect(selectedBefore, isNotEmpty);

      // Generation ran as write-ahead jobs: one reconciled intent per candidate.
      final intents = Directory(_path(target.path, '.kineto', 'jobs', 'intents'))
          .listSync()
          .whereType<File>()
          .map((file) => jsonDecode(file.readAsStringSync()) as Map<String, dynamic>)
          .toList();
      expect(intents, hasLength(3));
      expect(intents.map((intent) => intent['state']), everyElement('reconciled'));
      project.close();

      final cache = Directory(_path(target.path, '.kineto'))..createSync();
      File(_path(cache.path, 'project.db')).writeAsStringSync('derived only');
      cache.deleteSync(recursive: true);

      project = await KinetoProjectSession.open(target.path);
      final after = project.shotSnapshot(0);
      final selectedAfter =
          (jsonDecode(selectionFile.readAsStringSync()) as Map<String, dynamic>)[
              'selected_artifact_id'
          ] as String;

      expect(after.direction, before.direction);
      expect(after.generation, before.generation);
      expect(after.selectedIndex, before.selectedIndex);
      expect(after.locked, isTrue);
      expect(after.stale, isFalse);
      expect(after.supersededCount, before.supersededCount);
      expect(selectedAfter, selectedBefore);
      project.close();
    } finally {
      sandbox.deleteSync(recursive: true);
    }
  });

  test('direction change preserves stale lineage until regeneration', () async {
    final sandbox = Directory.systemTemp.createTempSync('kineto-project-ffi-');
    final target = Directory(_path(sandbox.path, 'film'));

    try {
      final project = await _createProject(target, 'ffi_project_stale');
      await project.generateShotCandidates(0);
      await project.selectShotCandidate(0, 1);
      final selectedBefore = project.shotSnapshot(0).selectedIndex;

      await project.setShotDirection(0, KinetoShotDirection.intimacy);
      final stale = project.shotSnapshot(0);
      expect(stale.generated, isTrue);
      expect(stale.stale, isTrue);
      expect(stale.selectedIndex, selectedBefore);
      await expectLater(project.lockShotSelection(0),
        throwsA(
          isA<KinetoProjectException>().having(
            (error) => error.code,
            'code',
            114,
          ),
        ),
      );

      await project.generateShotCandidates(0);
      final regenerated = project.shotSnapshot(0);
      expect(regenerated.stale, isFalse);
      expect(regenerated.selectedIndex, isNull);
      expect(regenerated.generation, 2);
      expect(regenerated.supersededCount, 3);
      project.close();
    } finally {
      sandbox.deleteSync(recursive: true);
    }
  });

  test('two shots keep independent canonical approval state', () async {
    final sandbox = Directory.systemTemp.createTempSync('kineto-project-ffi-');
    final target = Directory(_path(sandbox.path, 'film'));

    try {
      var project = await _createProject(target, 'ffi_project_two_shots');
      await project.setShotDirection(0, KinetoShotDirection.tension);
      await project.generateShotCandidates(0);
      await project.selectShotCandidate(0, 2);
      await project.lockShotSelection(0);

      await project.setShotDirection(1, KinetoShotDirection.intimacy);
      await project.generateShotCandidates(1);
      await project.selectShotCandidate(1, 1);
      await project.generateShotCandidates(1);
      await project.selectShotCandidate(1, 0);
      await project.lockShotSelection(1);
      project.close();

      project = await KinetoProjectSession.open(target.path);
      final shotOne = project.shotSnapshot(0);
      final shotTwo = project.shotSnapshot(1);
      expect(shotOne.direction, KinetoShotDirection.tension);
      expect(shotOne.generation, 1);
      expect(shotOne.selectedIndex, 2);
      expect(shotOne.locked, isTrue);
      expect(shotOne.supersededCount, 0);
      expect(shotTwo.direction, KinetoShotDirection.intimacy);
      expect(shotTwo.generation, 2);
      expect(shotTwo.selectedIndex, 0);
      expect(shotTwo.locked, isTrue);
      expect(shotTwo.supersededCount, 3);
      project.close();
    } finally {
      sandbox.deleteSync(recursive: true);
    }
  });

  test('shot index is validated before entering native code', () async {
    final sandbox = Directory.systemTemp.createTempSync('kineto-project-ffi-');
    final target = Directory(_path(sandbox.path, 'film'));

    try {
      final project = await _createProject(target, 'ffi_project_bounds');
      expect(() => project.shotSnapshot(-1), throwsRangeError);
      expect(
        () => project.shotSnapshot(KinetoProjectSession.shotCount),
        throwsRangeError,
      );
      project.close();
    } finally {
      sandbox.deleteSync(recursive: true);
    }
  });

  test('canonical project refuses a created_at the schema would reject', () async {
    final sandbox = Directory.systemTemp.createTempSync('kineto-project-ffi-');
    final target = Directory(_path(sandbox.path, 'film'));

    try {
      await expectLater(
        KinetoProjectSession.createTextProject(
          path: target.path,
          projectId: 'ffi_project_003',
          title: 'Bad Timestamp',
          createdAt: 'yesterday',
          language: 'en',
          sourceText: 'source',
        ),
        throwsA(
          isA<KinetoProjectException>().having(
            (error) => error.code,
            'code',
            104,
          ),
        ),
      );
      expect(target.existsSync(), isFalse);
    } finally {
      sandbox.deleteSync(recursive: true);
    }
  });

  test('canonical project create refuses to replace an existing target', () async {
    final sandbox = Directory.systemTemp.createTempSync('kineto-project-ffi-');
    final target = Directory(_path(sandbox.path, 'film'))..createSync();

    try {
      await expectLater(
        KinetoProjectSession.createTextProject(
          path: target.path,
          projectId: 'ffi_project_002',
          title: 'Existing',
          createdAt: '2026-09-10T08:00:00Z',
          language: 'en',
          sourceText: 'source',
        ),
        throwsA(
          isA<KinetoProjectException>().having(
            (error) => error.code,
            'code',
            103,
          ),
        ),
      );
    } finally {
      sandbox.deleteSync(recursive: true);
    }
  });
}

Future<KinetoProjectSession> _createProject(
  Directory target,
  String projectId,
) =>
    KinetoProjectSession.createTextProject(
      path: target.path,
      projectId: projectId,
      title: 'FFI Project',
      createdAt: '2026-09-10T08:00:00Z',
      language: 'en',
      sourceText: 'A filmmaker waits across the table.\n',
    );

String _path(String first, [
  String? second,
  String? third,
  String? fourth,
  String? fifth,
  String? sixth,
]) {
  final parts = <String>[
    first,
    ?second,
    ?third,
    ?fourth,
    ?fifth,
    ?sixth,
  ];
  return parts.join(Platform.pathSeparator);
}
