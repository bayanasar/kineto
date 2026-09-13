import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/native/kineto_engine.dart';

void main() {
  test('reset deliberately discards a locked shot while retaining lineage', () {
    final sandbox = Directory.systemTemp.createTempSync('kineto-locked-reset-');
    final target = Directory(_path(sandbox.path, 'film'));

    try {
      var project = KinetoProjectSession.createTextProject(
        path: target.path,
        projectId: 'ffi_locked_reset',
        title: 'Locked Reset',
        createdAt: '2026-09-13T00:00:00Z',
        language: 'en',
        sourceText: 'source\n',
      );
      project.setShotDirection(0, KinetoShotDirection.tension);
      project.generateShotCandidates(0);
      project.selectShotCandidate(0, 2);
      project.lockShotSelection(0);
      expect(project.shotSnapshot(0).locked, isTrue);

      project.resetShot(0);
      final discarded = project.shotSnapshot(0);
      expect(discarded.generated, isFalse);
      expect(discarded.locked, isFalse);
      expect(discarded.selectedIndex, isNull);
      expect(discarded.direction, KinetoShotDirection.reaction);
      expect(discarded.generation, 1);
      expect(discarded.supersededCount, 3);
      project.close();

      project = KinetoProjectSession.open(target.path);
      final reopened = project.shotSnapshot(0);
      expect(reopened.generated, isFalse);
      expect(reopened.locked, isFalse);
      expect(reopened.selectedIndex, isNull);
      expect(reopened.direction, KinetoShotDirection.reaction);
      expect(reopened.generation, 1);
      expect(reopened.supersededCount, 3);
      project.close();
    } finally {
      sandbox.deleteSync(recursive: true);
    }
  });
}

String _path(String first, [String? second]) =>
    <String>[first, ?second].join(Platform.pathSeparator);
