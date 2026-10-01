import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/native/kineto_engine.dart';

/// A copy of `fixtures/projects/screenplay`: a story analysis and a two-scene
/// screenplay, written by the engine with a scripted model.
Directory _fixtureCopy(Directory sandbox) {
  final source = Directory(
    ['..', '..', 'fixtures', 'projects', 'screenplay'].join(Platform.pathSeparator),
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
  test('a screenplay is read, edited and locked through the native session', () async {
    final sandbox = Directory.systemTemp.createTempSync('kineto-screenplay-ffi-');
    try {
      final target = _fixtureCopy(sandbox);
      final session = await KinetoProjectSession.open(target.path);

      var documents = session.documents;
      expect(documents.sourceRevision, 1);
      expect(documents.analysis!.revision, 1);
      expect(documents.analysis!.characterNames, <String>['The Filmmaker', 'The Interviewer']);
      final screenplay = documents.screenplay!;
      expect(screenplay.revision, 1);
      expect(screenplay.title, 'The Last Line');
      expect(screenplay.cast.map((member) => member.id), <String>['filmmaker', 'interviewer']);
      expect(screenplay.scenes.map((scene) => scene.id), <String>['office_night', 'office_dawn']);
      final night = screenplay.scenes.first;
      expect(night.heading, 'INT. SMALL OFFICE - NIGHT');
      expect(night.setting, KinetoSceneSetting.interior);
      expect(night.location, 'small_office');
      expect(night.blocks[1].kind, KinetoBlockKind.dialogue);
      expect(night.blocks[1].character, 'interviewer');
      expect(night.blocks[1].parenthetical, 'quietly');
      expect(night.blocks[0].character, isNull);
      expect(documents.revisions.single.origin, KinetoRevisionOrigin.generated);
      expect(documents.revisions.single.provider, 'fake-llm');
      expect(documents.lockedRevision, isNull);

      // No language model was named for this test process.
      expect(session.languageModel.kind, KinetoLanguageModelKind.none);
      expect(session.languageModel.ready, isFalse);
      expect(session.documentCostMicros(KinetoDocumentKind.storyAnalysis), isNull);
      await expectLater(session.analyzeStory(), _refusedWith(KinetoProjectException.modelNotConfigured));

      final edited = <KinetoScreenplayBlock>[
        ...night.blocks.take(2),
        const KinetoScreenplayBlock(
          kind: KinetoBlockKind.dialogue,
          character: 'filmmaker',
          parenthetical: 'after a long pause',
          text: 'Ask me tomorrow.',
        ),
      ];
      await session.saveScene(
        basedOn: 1,
        sceneId: night.id,
        heading: night.heading,
        time: night.time,
        dramaticGoal: night.dramaticGoal,
        blocks: edited,
      );
      documents = session.documents;
      expect(documents.screenplay!.revision, 2);
      expect(documents.screenplay!.scenes.first.blocks, edited);
      expect(documents.revisions.last.origin, KinetoRevisionOrigin.edited);
      expect(documents.revisions.last.basedOn, 1);
      expect(documents.revisions.first.status, KinetoRevisionStatus.superseded);

      // An edit of a revision that is no longer the latest is refused.
      await expectLater(
        session.saveScene(
          basedOn: 1,
          sceneId: night.id,
          heading: night.heading,
          time: night.time,
          dramaticGoal: night.dramaticGoal,
          blocks: night.blocks,
        ),
        _refusedWith(KinetoProjectException.notLatest),
      );

      await session.lockScreenplay(2);
      expect(session.documents.screenplayLocked, isTrue);

      await session.importSource('The interview is over, and he answers.', originalName: 'draft-2.txt');
      documents = session.documents;
      expect(documents.sourceRevision, 2);
      expect(documents.analysisStale, isTrue);
      session.close();

      // Everything the director decided is in the project's files.
      final reopened = await KinetoProjectSession.open(target.path);
      documents = reopened.documents;
      expect(documents.lockedRevision, 2);
      expect(documents.screenplay!.scenes.first.blocks.last.text, 'Ask me tomorrow.');
      final markdown = File(
        [target.path, 'screenplay', 'screenplay.md'].join(Platform.pathSeparator),
      ).readAsStringSync();
      expect(markdown, contains('from screenplay_r0002'));
      expect(markdown, contains('Ask me tomorrow.'));
      reopened.close();
    } finally {
      sandbox.deleteSync(recursive: true);
    }
  });
}
