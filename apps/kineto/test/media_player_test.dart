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
}
