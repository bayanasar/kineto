import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:media_kit/media_kit.dart';
import 'package:media_kit_video/media_kit_video.dart';
import 'package:wabisabi/wabisabi.dart';

import 'kineto_controls.dart';
import 'media_frame.dart';

/// One sound playing: what Kineto asks of the engine that plays it.
abstract interface class KinetoSoundPlayback {
  Stream<Duration> get position;
  Stream<Duration> get duration;

  /// Fires when the sound has played to its end.
  Stream<bool> get completed;
  Future<void> stop();
  Future<void> dispose();
}

/// What plays media inside Kineto. The app plays with libmpv (media_kit);
/// a test gives a fake.
abstract interface class KinetoMediaEngine {
  /// Start playing the sound file at [path].
  Future<KinetoSoundPlayback> playSound(String path);

  /// A widget that plays the clip at [path], with play, pause and seeking.
  Widget video(String path);
}

/// Plays with libmpv through media_kit, on every desktop platform.
final class MediaKitEngine implements KinetoMediaEngine {
  const MediaKitEngine();

  /// Load libmpv; once, before the first player. Returns why it could not
  /// be loaded, or `null`.
  static String? initialize() {
    try {
      MediaKit.ensureInitialized();
      return null;
    } on Object catch (failure) {
      return 'Kineto plays sound and video with libmpv, which could not be loaded '
          "(on Debian or Ubuntu it is the package libmpv2): $failure";
    }
  }

  @override
  Future<KinetoSoundPlayback> playSound(String path) async {
    final player = Player(configuration: const PlayerConfiguration(title: 'Kineto'));
    await player.open(Media(Uri.file(path).toString()));
    return _MediaKitSound(player);
  }

  @override
  Widget video(String path) => _MediaKitVideo(path: path, key: ValueKey<String>('video-$path'));
}

/// Where libmpv could not be loaded: every play says why.
final class UnavailableMedia implements KinetoMediaEngine {
  const UnavailableMedia(this.problem);

  final String problem;

  @override
  Future<KinetoSoundPlayback> playSound(String path) => Future<KinetoSoundPlayback>.error(StateError(problem));

  @override
  Widget video(String path) => Center(
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: SelectableText(problem, style: const TextStyle(color: Colors.white70)),
        ),
      );
}

final class _MediaKitSound implements KinetoSoundPlayback {
  _MediaKitSound(this._player);

  final Player _player;

  @override
  Stream<Duration> get position => _player.stream.position;

  @override
  Stream<Duration> get duration => _player.stream.duration;

  @override
  Stream<bool> get completed => _player.stream.completed;

  @override
  Future<void> stop() => _player.stop();

  @override
  Future<void> dispose() => _player.dispose();
}

class _MediaKitVideo extends StatefulWidget {
  const _MediaKitVideo({required this.path, super.key});

  final String path;

  @override
  State<_MediaKitVideo> createState() => _MediaKitVideoState();
}

class _MediaKitVideoState extends State<_MediaKitVideo> {
  late final Player _player = Player(configuration: const PlayerConfiguration(title: 'Kineto'));
  late final VideoController _controller = VideoController(_player);

  @override
  void initState() {
    super.initState();
    unawaited(_player.open(Media(Uri.file(widget.path).toString())));
  }

  @override
  void dispose() {
    unawaited(_player.dispose());
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => MaterialDesktopVideoControlsTheme(
        normal: MaterialDesktopVideoControlsThemeData(
          seekBarPositionColor: WabTheme.accentColor,
          seekBarThumbColor: WabTheme.accentColor,
          toggleFullscreenOnDoublePress: false,
          bottomButtonBar: const <Widget>[
            MaterialDesktopSkipPreviousButton(),
            MaterialDesktopPlayOrPauseButton(),
            MaterialDesktopVolumeButton(),
            MaterialDesktopPositionIndicator(),
            Spacer(),
          ],
        ),
        fullscreen: const MaterialDesktopVideoControlsThemeData(),
        child: Video(controller: _controller, controls: MaterialDesktopVideoControls),
      );
}

/// The sound playing in the app: one at a time, as a director listens to
/// takes one after another.
class KinetoSound extends ChangeNotifier {
  KinetoSound(this.engine);

  final KinetoMediaEngine engine;

  KinetoSoundPlayback? _playback;
  final List<StreamSubscription<Object>> _listening = <StreamSubscription<Object>>[];
  String? _playing;
  Duration _position = Duration.zero;
  Duration _duration = Duration.zero;
  String? _problem;

  /// The file playing, if any.
  String? get playing => _playing;
  Duration get position => _position;
  Duration get duration => _duration;

  /// Why the last sound could not be played.
  String? get problem => _problem;

  /// The file that could not be played, if the last one could not.
  String? get failed => _failed;
  String? _failed;

  /// Play [path], or stop it if it is playing; another sound playing stops.
  Future<void> toggle(String path) async {
    final again = _playing == path;
    await stop();
    if (again) return;
    _playing = path;
    _problem = null;
    _failed = null;
    notifyListeners();
    try {
      final playback = await engine.playSound(path);
      if (_playing != path) {
        await playback.dispose();
        return;
      }
      _playback = playback;
      _listening.addAll(<StreamSubscription<Object>>[
        playback.position.listen((position) {
          _position = position;
          notifyListeners();
        }),
        playback.duration.listen((duration) {
          _duration = duration;
          notifyListeners();
        }),
        playback.completed.listen((done) {
          if (done) unawaited(stop());
        }),
      ]);
    } on Object catch (failure) {
      _playing = null;
      _failed = path;
      _problem = failure is StateError ? failure.message : 'The sound could not be played: $failure';
      notifyListeners();
    }
  }

  Future<void> stop() async {
    final playback = _playback;
    _playback = null;
    for (final listening in _listening) {
      unawaited(listening.cancel());
    }
    _listening.clear();
    final wasPlaying = _playing != null;
    _playing = null;
    _position = Duration.zero;
    _duration = Duration.zero;
    if (playback != null) {
      await playback.stop();
      await playback.dispose();
    }
    if (wasPlaying) notifyListeners();
  }

  bool _disposed = false;

  // A sound may still be stopping, or starting, when the app closes; what
  // it finishes after that is not announced.
  @override
  void notifyListeners() {
    if (!_disposed) super.notifyListeners();
  }

  @override
  void dispose() {
    _disposed = true;
    unawaited(stop());
    super.dispose();
  }
}

/// Makes the app's media engine and its sound reachable from any page.
class KinetoMedia extends InheritedNotifier<KinetoSound> {
  const KinetoMedia({required KinetoSound sound, required super.child, super.key}) : super(notifier: sound);

  static KinetoSound? maybeOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<KinetoMedia>()?.notifier;
}

String _clock(Duration duration) {
  final seconds = duration.inSeconds;
  return '${seconds ~/ 60}:${(seconds % 60).toString().padLeft(2, '0')}';
}

/// Plays a sound in place: “Play”, then “Stop · 0:02 / 0:04” while it plays.
class KinetoSoundButton extends StatelessWidget {
  const KinetoSoundButton({required this.path, this.primary = false, super.key});

  final String path;
  final bool primary;

  @override
  Widget build(BuildContext context) {
    final sound = KinetoMedia.maybeOf(context);
    final playing = sound?.playing == path;
    final button = KinetoButton(
      primary: primary || playing,
      onPressed: sound == null ? null : () => unawaited(sound.toggle(path)),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: <Widget>[
          Icon(playing ? Icons.stop : Icons.play_arrow),
          const SizedBox(width: 4),
          Text(playing ? 'Stop · ${_clock(sound!.position)} / ${_clock(sound.duration)}' : 'Play'),
        ],
      ),
    );
    final problem = sound?.failed == path ? sound?.problem : null;
    if (problem == null) return button;
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: <Widget>[
        button,
        const SizedBox(width: 8),
        Flexible(child: Text(problem, style: TextStyle(color: WabTheme.mutedColor))),
      ],
    );
  }
}

/// A clip's first frame with a play mark; a click plays the clip in Kineto.
class KinetoVideoTile extends StatelessWidget {
  const KinetoVideoTile({required this.path, this.poster, this.label, super.key});

  /// The clip.
  final String path;

  /// The image shown until it plays, such as its keyframe.
  final String? poster;
  final String? label;

  @override
  Widget build(BuildContext context) => MouseRegion(
        cursor: SystemMouseCursors.click,
        child: GestureDetector(
          onTap: () => showVideoPlayer(context, path, title: label),
          child: AspectRatio(
            aspectRatio: 16 / 9,
            child: Stack(
              fit: StackFit.expand,
              children: <Widget>[
                if (poster != null)
                  // The click plays the clip rather than enlarging the frame.
                  IgnorePointer(
                    child: MediaFrame(path: poster!, aspectRatio: 16 / 9, unavailable: 'Keyframe unavailable'),
                  )
                else
                  const ColoredBox(color: Colors.black),
                Center(
                  child: DecoratedBox(
                    decoration: BoxDecoration(color: Colors.black.withValues(alpha: .55), shape: BoxShape.circle),
                    child: const Padding(
                      padding: EdgeInsets.all(10),
                      child: Icon(Icons.play_arrow, color: Colors.white, size: 34),
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
      );
}

/// Play the clip at [path] over the window, inside Kineto. Esc closes it.
Future<void> showVideoPlayer(BuildContext context, String path, {String? title}) {
  final sound = KinetoMedia.maybeOf(context);
  if (sound == null) return Future<void>.value();
  unawaited(sound.stop());
  return showGeneralDialog<void>(
    context: context,
    barrierDismissible: true,
    barrierLabel: 'Close the video',
    barrierColor: Colors.black.withValues(alpha: .9),
    transitionDuration: const Duration(milliseconds: 120),
    pageBuilder: (context, animation, secondaryAnimation) => CallbackShortcuts(
      bindings: <ShortcutActivator, VoidCallback>{
        const SingleActivator(LogicalKeyboardKey.escape): () => Navigator.of(context).pop(),
      },
      child: Focus(
        autofocus: true,
        child: Material(
          type: MaterialType.transparency,
          child: SafeArea(
            child: Padding(
              padding: const EdgeInsets.all(24),
              child: Column(
                children: <Widget>[
                  Row(
                    children: <Widget>[
                      Expanded(
                        child: Text(
                          title ?? fileName(path),
                          style: const TextStyle(color: Colors.white70, fontSize: 14),
                          overflow: TextOverflow.ellipsis,
                        ),
                      ),
                      IconButton(
                        key: const ValueKey<String>('video-close'),
                        tooltip: 'Close (Esc)',
                        onPressed: () => Navigator.of(context).pop(),
                        icon: const Icon(Icons.close, color: Colors.white70),
                      ),
                    ],
                  ),
                  const SizedBox(height: 8),
                  Expanded(child: Center(child: AspectRatio(aspectRatio: 16 / 9, child: sound.engine.video(path)))),
                ],
              ),
            ),
          ),
        ),
      ),
    ),
  );
}
