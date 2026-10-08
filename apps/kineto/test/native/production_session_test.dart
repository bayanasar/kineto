import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/native/kineto_engine.dart';

/// A copy of `fixtures/projects/production`: a locked screenplay, a bible not
/// yet locked, two style samples of which the first is locked, a filmmaker
/// cast from the second of two portraits and its reference pack, and an
/// interviewer with one portrait candidate. Written by the engine with a
/// scripted model and an offline MiniMax.
Directory _fixtureCopy(Directory sandbox) {
  final source = Directory(
    ['..', '..', 'fixtures', 'projects', 'production'].join(Platform.pathSeparator),
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
  test('the bible, the style and the cast are read and decided through the native session', () async {
    final sandbox = Directory.systemTemp.createTempSync('kineto-production-ffi-');
    try {
      final target = _fixtureCopy(sandbox);
      final session = await KinetoProjectSession.open(target.path);

      var production = session.production;
      final bible = production.bible!;
      expect(bible.revision, 1);
      expect(production.bibleLocked, isNull);
      expect(production.bibleStale, isEmpty);
      expect(production.bibleRevisions.single.provider, 'fake-llm');
      expect(bible.setting, startsWith('A city at night'));
      expect(bible.characters.map((entry) => entry.id), <String>['filmmaker', 'interviewer']);
      final filmmakerEntry = bible.characters.first;
      expect(filmmakerEntry.name, 'The Filmmaker');
      expect(filmmakerEntry.forbiddenChanges, <String>['Grey at the temples.', 'Never without the coat.']);
      expect(bible.locations.single.id, 'small_office');
      expect(bible.props.single.id, 'notebook');

      expect(production.presets.map((preset) => preset.id), contains('film_noir'));
      expect(production.samples.map((sample) => sample.id), <String>['style_s0001', 'style_s0002']);
      expect(production.samples.first.status, KinetoChoiceStatus.selected);
      expect(production.samples.last.status, KinetoChoiceStatus.candidate);
      expect(production.samples.first.provider, 'minimax');
      expect(File(production.samples.first.mediaPath).existsSync(), isTrue);
      expect(production.style!.sampleId, 'style_s0001');
      expect(production.style!.preset, 'film_noir');
      expect(production.styleUnresolved, isFalse);

      expect(production.characters.map((character) => character.id), <String>['filmmaker', 'interviewer']);
      var filmmaker = production.characters.first;
      expect(filmmaker.name, 'The Filmmaker');
      expect(filmmaker.portraits.map((image) => image.id), <String>['cast.filmmaker.0001', 'cast.filmmaker.0002']);
      expect(filmmaker.selectedPortrait!.id, 'cast.filmmaker.0002');
      expect(filmmaker.pack.keys, hasLength(7));
      expect(filmmaker.pack[KinetoCastImageKind.front]!.identityReference, 'cast.filmmaker.0002');
      expect(File(filmmaker.pack[KinetoCastImageKind.fullBody]!.mediaPath).existsSync(), isTrue);
      expect(filmmaker.lockedPortrait, 'cast.filmmaker.0002');
      expect(filmmaker.lockedReferences, hasLength(7));
      expect(filmmaker.stale, isEmpty);
      final interviewer = production.characters.last;
      expect(interviewer.portraits.single.status, KinetoChoiceStatus.candidate);
      expect(interviewer.locked, isFalse);

      // No image provider or language model is named for this test process.
      expect(session.imageCost(KinetoImageCostKind.sample), isNull);
      expect(session.documentCostMicros(KinetoDocumentKind.bible), isNull);
      await expectLater(session.audition('interviewer', 1), _refusedWith(KinetoProjectException.imageProviderUnsuitable));
      await expectLater(session.writeBible(), _refusedWith(KinetoProjectException.modelNotConfigured));

      // The director's free decisions.
      await session.rejectStyle('style_s0002');
      await expectLater(session.selectStyle('style_s0002'), _refusedWith(KinetoProjectException.rejected));
      await expectLater(session.lockStyle('style_s0009'), _refusedWith(KinetoProjectException.noSuchArtifact));
      await expectLater(session.lockCharacter('interviewer'), _refusedWith(KinetoProjectException.needsSelectedPortrait));
      await session.selectPortrait('interviewer', 'cast.interviewer.0001');
      // The director's own photo comes in as a free portrait candidate; a
      // path that is not an image is refused.
      final photo = File([sandbox.path, 'actor.jpg'].join(Platform.pathSeparator))
        ..writeAsBytesSync(<int>[0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, 0x49, 0x46, 0x42]);
      final note = File([sandbox.path, 'note.txt'].join(Platform.pathSeparator))..writeAsStringSync('a note');
      await session.importPortrait('interviewer', photo.path);
      await expectLater(
        session.importPortrait('interviewer', note.path),
        _refusedWith(KinetoProjectException.noIdentityReference),
      );
      final imported = session.production.characters.last.portraits.last;
      expect(imported.status, KinetoChoiceStatus.candidate);
      expect(File(imported.mediaPath).readAsBytesSync(), photo.readAsBytesSync());
      // A mood board comes in as a free style sample, with the director's
      // description, which prompts follow once it is locked.
      final samples = session.production.samples.length;
      await session.importStyle(photo.path, 'Teal shadows, amber practicals, heavy grain');
      final board = session.production.samples.last;
      expect(session.production.samples, hasLength(samples + 1));
      expect(board.imported, isTrue);
      expect(board.directives, 'Teal shadows, amber practicals, heavy grain');
      expect(board.sceneId, isEmpty);
      expect(board.status, KinetoChoiceStatus.candidate);
      expect(File(board.mediaPath).readAsBytesSync(), photo.readAsBytesSync());
      await expectLater(session.importStyle(photo.path, ' '), _refusedWith(KinetoProjectException.invalidArgument));
      await expectLater(
        session.importStyle(note.path, 'A note'),
        _refusedWith(KinetoProjectException.styleReferenceUnusable),
      );
      expect(session.production.samples, hasLength(samples + 1));
      // A cast is locked for the provider that keeps its identity, which
      // the local demo cannot.
      await expectLater(session.lockCharacter('interviewer'), _refusedWith(KinetoProjectException.noIdentityReference));
      await session.rejectPortrait('filmmaker', 'cast.filmmaker.0001');
      await expectLater(
        session.rejectPortrait('filmmaker', 'cast.filmmaker.0002'),
        _refusedWith(KinetoProjectException.locked),
      );
      production = session.production;
      expect(
        production.samples.firstWhere((sample) => sample.id == 'style_s0002').status,
        KinetoChoiceStatus.rejected,
      );
      filmmaker = production.characters.first;
      expect(filmmaker.portraits.first.status, KinetoChoiceStatus.rejected);
      expect(production.characters.last.selectedPortrait!.id, 'cast.interviewer.0001');

      await session.saveBibleEntry(
        1,
        const KinetoBibleEntryEdit(
          kind: KinetoBibleEntryKind.character,
          id: 'filmmaker',
          description: 'A documentary filmmaker in his fifties.',
          appearance: 'Grey at the temples, a scar over one eyebrow.',
          wardrobe: 'A worn grey overcoat.',
          forbiddenChanges: <String>['Never without the coat.'],
        ),
      );
      await expectLater(
        session.saveBibleEntry(
          1,
          const KinetoBibleEntryEdit(
            kind: KinetoBibleEntryKind.prop,
            id: 'notebook',
            description: 'A notebook.',
            appearance: 'Black.',
          ),
        ),
        _refusedWith(KinetoProjectException.notLatest),
      );
      await expectLater(
        session.saveBibleEntry(
          2,
          const KinetoBibleEntryEdit(
            kind: KinetoBibleEntryKind.location,
            id: 'small_office',
            description: 'An office.',
            appearance: '',
          ),
        ),
        _refusedWith(KinetoProjectException.invalidDocument),
      );
      await session.lockBible(2);
      production = session.production;
      expect(production.bible!.revision, 2);
      expect(production.bibleIsLocked, isTrue);
      expect(production.bibleRevisions.last.origin, KinetoRevisionOrigin.edited);
      expect(production.bible!.characters.first.forbiddenChanges, <String>['Never without the coat.']);
      // The cast was made from the entry the director just changed.
      expect(production.characters.first.stale, <String>['bible_character.filmmaker']);

      // No plan is locked: the bootstrap scene's shots are in play.
      expect(session.shotCount, 2);
      expect(session.shotId(1), 'shot_002');
      expect(session.shotSceneId(1), 'scene_001');

      // Keyframes go on with what the director accepted.
      await session.generateShotCandidatesConsenting(0, identityDrift: true);
      session.close();

      // Every lock is in the project's files.
      Directory([target.path, '.kineto'].join(Platform.pathSeparator)).deleteSync(recursive: true);
      final reopened = await KinetoProjectSession.open(target.path);
      production = reopened.production;
      expect(production.bibleLocked, 2);
      expect(production.style!.sampleId, 'style_s0001');
      expect(production.characters.first.lockedPortrait, 'cast.filmmaker.0002');
      expect(production.characters.first.stale, <String>['bible_character.filmmaker']);
      expect(production.characters.last.selectedPortrait!.id, 'cast.interviewer.0001');
      reopened.close();
    } finally {
      sandbox.deleteSync(recursive: true);
    }
  });
}
