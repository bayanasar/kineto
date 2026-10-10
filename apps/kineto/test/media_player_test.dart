import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/media_player.dart';

import 'fake_media.dart';

void main() {
  test('closing while a sound plays stops it without announcing anything', () async {
    final media = FakeMedia();
    final sound = KinetoSound(media);
    await sound.toggle('take.wav');
    expect(sound.playing, 'take.wav');
    // The player is still stopping when the app is gone.
    sound.dispose();
    await pumpEventQueue();
    expect(media.sounds.single.stopped, isTrue);
  });

  test('closing while the last sound stops starts no other', () async {
    final media = FakeMedia();
    final sound = KinetoSound(media);
    await sound.toggle('take.wav');
    // The next sound waits for the first to stop; the app closes meanwhile.
    final next = sound.toggle('line.wav');
    sound.dispose();
    await next;
    expect(media.played, <String>['take.wav']);
    expect(sound.playing, isNull);
  });

  test('closing while a sound opens releases it once it has opened', () async {
    final media = FakeMedia()..opening = Completer<void>();
    final sound = KinetoSound(media);
    final started = sound.toggle('take.wav');
    await pumpEventQueue();
    expect(media.played, <String>['take.wav']);
    sound.dispose();
    media.opening!.complete();
    await started;
    expect(media.sounds.single.disposed, isTrue);
    expect(sound.playing, isNull);
  });
}
