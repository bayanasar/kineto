import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/native/kineto_engine.dart';

/// A copy of `fixtures/projects/planned`: the screenplay, bible and shot
/// plan locked, the filmmaker cast and the interviewer not. Its one line,
/// "Was it worth it?", is the interviewer's.
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

/// Stand in a cast lock for the interviewer, made from the filmmaker's: the
/// demo cannot make portraits, and a voice reads only the lock's hash.
void _castInterviewer(Directory film) {
  String path(String character) =>
      [film.path, 'characters', character, 'character.json'].join(Platform.pathSeparator);
  final lock = jsonDecode(File(path('filmmaker')).readAsStringSync()) as Map<String, dynamic>;
  lock['character_id'] = 'interviewer';
  lock['name'] = 'The Interviewer';
  File(path('interviewer')).writeAsStringSync(jsonEncode(lock));
}

Matcher _refusedWith(int code) =>
    throwsA(isA<KinetoProjectException>().having((error) => error.code, 'code', code));

void main() {
  test('voices are cast, a line dubbed and preferences kept through the native session', () async {
    final sandbox = Directory.systemTemp.createTempSync('kineto-voices-ffi-');
    try {
      final target = _fixtureCopy(sandbox);
      var session = await KinetoProjectSession.open(target.path);

      var voices = session.voices;
      expect(voices.speechProvider, KinetoSpeechProviderKind.localDemo);
      expect(voices.speechReady, isTrue);
      expect(voices.characterCostMicros, 0);
      expect(voices.voices, isEmpty, reason: 'listed only when asked');
      expect(voices.characters.map((character) => character.characterId), <String>['filmmaker', 'interviewer']);
      final filmmaker = voices.character('filmmaker')!;
      expect(filmmaker.characterName, isNotEmpty);
      expect(filmmaker.defaultLine, isNotEmpty);
      expect(filmmaker.locked, isFalse);
      final line = voices.lines.single;
      expect((line.sceneId, line.number, line.characterId, line.text), ('office_night', 1, 'interviewer', 'Was it worth it?'));
      expect(line.durationSource, KinetoDurationSource.estimated);
      expect(line.durationMillis, greaterThan(0));
      expect(line.takeCostMicros, 0);
      expect(voices.preferences, isEmpty);
      expect(voices.preferencesRevision, isNull);

      // The plan editor reads how long the second shot's line takes.
      final shots = session.planning.scenes.single.shots;
      expect(shots.first.dialogueMillis, 0);
      expect(shots[1].dialogueMillis, line.durationMillis);
      expect(shots[1].tooShort, isFalse);

      // The demo's voices, two auditioned for the filmmaker; the interviewer
      // is not cast yet, so cannot be voiced.
      await session.loadVoices();
      voices = session.voices;
      expect(voices.voices.length, greaterThanOrEqualTo(2));
      expect(voices.voicesProblem, isNull);
      expect(session.speechCostMicros(filmmaker.defaultLine, count: 2), 0);
      final choices = <KinetoVoiceChoice>[
        for (final voice in voices.voices.take(2))
          KinetoVoiceChoice(voiceId: voice.id, name: voice.name, emotion: KinetoEmotion.calm),
      ];
      await session.auditionVoices('filmmaker', choices);
      await expectLater(
        session.auditionVoices('interviewer', choices),
        _refusedWith(KinetoProjectException.needsLockedCast),
      );
      var cast = session.voices.character('filmmaker')!;
      expect(cast.auditions.map((audition) => audition.voiceId), choices.map((choice) => choice.voiceId));
      expect(cast.auditions.every((audition) => File(audition.mediaPath).existsSync()), isTrue);
      expect(cast.auditions.first.line, filmmaker.defaultLine);
      expect(cast.auditions.first.emotion, 'calm');
      expect(cast.auditions.first.status, KinetoChoiceStatus.candidate);

      await session.selectVoice('filmmaker', cast.auditions[1].id);
      await session.lockVoice('filmmaker');
      cast = session.voices.character('filmmaker')!;
      expect(cast.locked, isTrue);
      expect(cast.lockedAudition, cast.auditions[1].id);
      expect(cast.lockedVoiceId, choices[1].voiceId);
      expect(cast.stale, isEmpty);

      // The interviewer, once cast, is auditioned saying the line, and
      // dubbing it waits for the voice to be locked.
      _castInterviewer(target);
      await session.auditionVoices('interviewer', choices.take(1).toList(), line: 'Was it worth it?');
      final interviewer = session.voices.character('interviewer')!;
      expect(interviewer.auditions.single.line, 'Was it worth it?');
      await expectLater(
        session.dubLine('office_night', 1),
        _refusedWith(KinetoProjectException.needsLockedVoice),
      );
      await session.selectVoice('interviewer', interviewer.auditions.single.id);
      await session.lockVoice('interviewer');

      // Two takes, one chosen and locked: the line is measured now.
      await session.dubLine('office_night', 1, takes: 2, delivery: KinetoEmotion.sad, note: 'Barely said.');
      var dubbed = session.voices.lines.single;
      expect(dubbed.takes.length, 2);
      expect(dubbed.takes.first.note, 'Barely said.');
      expect(dubbed.takes.first.emotion, 'sad');
      expect(dubbed.takes.every((take) => File(take.mediaPath).existsSync()), isTrue);
      await session.rejectTake('office_night', dubbed.takes.first.id);
      await session.selectTake('office_night', dubbed.takes[1].id);
      await session.lockLine('office_night', 1);
      dubbed = session.voices.lines.single;
      expect(dubbed.takes.first.status, KinetoChoiceStatus.rejected);
      expect(dubbed.lockedTake, dubbed.takes[1].id);
      expect(dubbed.durationSource, KinetoDurationSource.measured);
      expect(dubbed.durationMillis, dubbed.takes[1].durationMillis);

      // The director's preferences: written, then edited from the profile
      // in force with another added.
      await session.savePreferences(null, const <KinetoPreference>[
        KinetoPreference(
          dimension: KinetoDimension.lighting,
          stance: KinetoStance.prefer,
          value: 'low-key',
          strengthPermille: 800,
        ),
      ]);
      voices = session.voices;
      expect(voices.preferencesRevision, 1);
      final kept = voices.preferences.single;
      expect((kept.dimension, kept.stance, kept.value, kept.strengthPermille), (
        KinetoDimension.lighting,
        KinetoStance.prefer,
        'low-key',
        800,
      ));
      expect(kept.fromSuggestion, isFalse);
      await session.savePreferences(1, <KinetoPreference>[
        kept.copyWith(value: 'low-key, one source', strengthPermille: () => null),
        const KinetoPreference(
          dimension: KinetoDimension.cameraMotion,
          stance: KinetoStance.avoid,
          value: 'handheld',
        ),
      ]);
      await expectLater(
        session.savePreferences(1, const <KinetoPreference>[]),
        _refusedWith(KinetoProjectException.notLatest),
      );
      voices = session.voices;
      expect(voices.preferencesRevision, 2);
      expect(voices.preferenceRevisions.map((revision) => revision.number), <int>[1, 2]);
      expect(voices.preferences.map((preference) => (preference.value, preference.strengthPermille)), <(String, int?)>[
        ('low-key, one source', null),
        ('handheld', null),
      ]);
      expect(voices.suggestions, isEmpty);
      final before = session.voices;
      session.close();

      // Every lock and the profile live in the project's files.
      Directory([target.path, '.kineto'].join(Platform.pathSeparator)).deleteSync(recursive: true);
      session = await KinetoProjectSession.open(target.path);
      final after = session.voices;
      expect(after.character('filmmaker')!.lockedAudition, before.character('filmmaker')!.lockedAudition);
      expect(after.lines.single.lockedTake, before.lines.single.lockedTake);
      expect(after.preferences.map((preference) => preference.value), before.preferences.map((preference) => preference.value));
      session.close();
    } finally {
      sandbox.deleteSync(recursive: true);
    }
  });
}
