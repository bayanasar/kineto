import 'dart:async';

import 'package:flutter/widgets.dart';
import 'package:kineto/media_player.dart';

/// Plays nothing: records what was asked to play, and lets a test end a
/// sound.
final class FakeMedia implements KinetoMediaEngine {
  final List<String> played = <String>[];
  final List<FakeSound> sounds = <FakeSound>[];

  /// While set, a sound starts only once it completes.
  Completer<void>? opening;

  @override
  Future<KinetoSoundPlayback> playSound(String path) async {
    played.add(path);
    await opening?.future;
    final sound = FakeSound();
    sounds.add(sound);
    return sound;
  }

  @override
  Widget video(String path) => Text('playing $path', key: const ValueKey<String>('fake-video'));
}

final class FakeSound implements KinetoSoundPlayback {
  final StreamController<Duration> positions = StreamController<Duration>.broadcast();
  final StreamController<Duration> durations = StreamController<Duration>.broadcast();
  final StreamController<bool> ends = StreamController<bool>.broadcast();
  bool stopped = false;
  bool disposed = false;

  @override
  Stream<Duration> get position => positions.stream;

  @override
  Stream<Duration> get duration => durations.stream;

  @override
  Stream<bool> get completed => ends.stream;

  @override
  Future<void> stop() async => stopped = true;

  @override
  Future<void> dispose() async => disposed = true;
}

/// [child] with a sound that plays through [media].
Widget withMedia(FakeMedia media, Widget child) => KinetoMedia(sound: KinetoSound(media), child: child);
