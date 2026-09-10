import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/native/kineto_engine.dart';

void main() {
  test('canonical project create and open round-trip through Code Asset', () {
    final sandbox = Directory.systemTemp.createTempSync('kineto-project-ffi-');
    final target = Directory(_path(sandbox.path, 'film'));

    try {
      final created = _createProject(target, 'ffi_project_001');

      expect(created.formatVersion, 1);
      expect(created.isReadOnly, isFalse);
      expect(created.projectId, 'ffi_project_001');
      expect(created.title, 'FFI Project');
      expect(File(_path(target.path, 'project.toml')).existsSync(), isTrue);
      expect(
        File(_path(target.path, 'source', 'story.txt')).readAsStringSync(),
        'A filmmaker waits across the table.\n',
      );

      created.close();
      expect(() => created.title, throwsStateError);
      expect(() => created.shotSnapshot(0), throwsStateError);
      created.close();

      final reopened = KinetoProjectSession.open(target.path);
      expect(reopened.formatVersion, 1);
      expect(reopened.isReadOnly, isFalse);
      expect(reopened.projectId, 'ffi_project_001');
      expect(reopened.title, 'FFI Project');
      reopened.close();
    } finally {
      sandbox.deleteSync(recursive: true);
    }
  });

  test('shot approval survives reopen and deletion of .kineto cache', () {
    final sandbox = Directory.systemTemp.createTempSync('kineto-project-ffi-');
    final target = Directory(_path(sandbox.path, 'film'));

    try {
      var project = _createProject(target, 'ffi_project_durable');
      project.setShotDirection(0, KinetoShotDirection.tension);
      project.generateShotCandidates(0);
      project.selectShotCandidate(0, 2);
      project.lockShotSelection(0);

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
      project.close();

      final cache = Directory(_path(target.path, '.kineto'))..createSync();
      File(_path(cache.path, 'project.db')).writeAsStringSync('derived only');
      cache.deleteSync(recursive: true);

      project = KinetoProjectSession.open(target.path);
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

  test('direction change preserves stale lineage until regeneration', () {
    final sandbox = Directory.systemTemp.createTempSync('kineto-project-ffi-');
    final target = Directory(_path(sandbox.path, 'film'));

    try {
      final project = _createProject(target, 'ffi_project_stale');
      project.generateShotCandidates(0);
      project.selectShotCandidate(0, 1);
      final selectedBefore = project.shotSnapshot(0).selectedIndex;

      project.setShotDirection(0, KinetoShotDirection.intimacy);
      final stale = project.shotSnapshot(0);
      expect(stale.generated, isTrue);
      expect(stale.stale, isTrue);
      expect(stale.selectedIndex, selectedBefore);
      expect(
        () => project.lockShotSelection(0),
        throwsA(
          isA<KinetoProjectException>().having(
            (error) => error.code,
            'code',
            114,
          ),
        ),
      );

      project.generateShotCandidates(0);
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

  test('two shots keep independent canonical approval state', () {
    final sandbox = Directory.systemTemp.createTempSync('kineto-project-ffi-');
    final target = Directory(_path(sandbox.path, 'film'));

    try {
      var project = _createProject(target, 'ffi_project_two_shots');
      project.setShotDirection(0, KinetoShotDirection.tension);
      project.generateShotCandidates(0);
      project.selectShotCandidate(0, 2);
      project.lockShotSelection(0);

      project.setShotDirection(1, KinetoShotDirection.intimacy);
      project.generateShotCandidates(1);
      project.selectShotCandidate(1, 1);
      project.generateShotCandidates(1);
      project.selectShotCandidate(1, 0);
      project.lockShotSelection(1);
      project.close();

      project = KinetoProjectSession.open(target.path);
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

  test('shot index is validated before entering native code', () {
    final sandbox = Directory.systemTemp.createTempSync('kineto-project-ffi-');
    final target = Directory(_path(sandbox.path, 'film'));

    try {
      final project = _createProject(target, 'ffi_project_bounds');
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

  test('canonical project refuses a created_at the schema would reject', () {
    final sandbox = Directory.systemTemp.createTempSync('kineto-project-ffi-');
    final target = Directory(_path(sandbox.path, 'film'));

    try {
      expect(
        () => KinetoProjectSession.createTextProject(
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

  test('canonical project create refuses to replace an existing target', () {
    final sandbox = Directory.systemTemp.createTempSync('kineto-project-ffi-');
    final target = Directory(_path(sandbox.path, 'film'))..createSync();

    try {
      expect(
        () => KinetoProjectSession.createTextProject(
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

KinetoProjectSession _createProject(Directory target, String projectId) =>
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
    if (second != null) second,
    if (third != null) third,
    if (fourth != null) fourth,
    if (fifth != null) fifth,
    if (sixth != null) sixth,
  ];
  return parts.join(Platform.pathSeparator);
}
