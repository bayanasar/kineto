import 'dart:io';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'kineto_controls.dart';

/// An image on disk, or a plain frame saying it cannot be read. Clicking it
/// opens it over the window to look at closely, stepping through [gallery]
/// when it sits among others.
class MediaFrame extends StatefulWidget {
  const MediaFrame({
    required this.path,
    required this.aspectRatio,
    this.gallery = const <String>[],
    this.unavailable = 'Image unavailable',
    super.key,
  });

  final String path;
  final double aspectRatio;

  /// The images it sits among, in order, itself included.
  final List<String> gallery;

  final String unavailable;

  @override
  State<MediaFrame> createState() => _MediaFrameState();
}

class _MediaFrameState extends State<MediaFrame> {
  bool _hovered = false;

  void _open() {
    final gallery = widget.gallery.contains(widget.path) ? widget.gallery : <String>[widget.path];
    showMediaViewer(context, gallery, initial: gallery.indexOf(widget.path));
  }

  @override
  Widget build(BuildContext context) => Semantics(
        button: true,
        label: 'Enlarge ${fileName(widget.path)}',
        child: MouseRegion(
          cursor: SystemMouseCursors.zoomIn,
          onEnter: (_) => setState(() => _hovered = true),
          onExit: (_) => setState(() => _hovered = false),
          child: GestureDetector(
            onTap: _open,
            child: ClipRRect(
              borderRadius: BorderRadius.circular(2),
              child: AspectRatio(
                aspectRatio: widget.aspectRatio,
                child: LayoutBuilder(
                  builder: (context, constraints) {
                    // A thumbnail decodes small: twice its longer side, so
                    // covering a frame of another shape stays sharp.
                    final side = math.max(constraints.maxWidth, constraints.maxHeight);
                    final pixels = (2 * side * MediaQuery.devicePixelRatioOf(context)).round();
                    final file = FileImage(File(widget.path));
                    return Stack(
                      fit: StackFit.expand,
                      children: <Widget>[
                        Image(
                          image: side.isFinite && pixels > 0
                              ? ResizeImage(
                                  file,
                                  width: pixels,
                                  height: pixels,
                                  policy: ResizeImagePolicy.fit,
                                  allowUpscaling: false,
                                )
                              : file,
                          fit: BoxFit.cover,
                          errorBuilder: (context, error, stackTrace) => ColoredBox(
                            color: Colors.black26,
                            child: Center(child: Text(widget.unavailable)),
                          ),
                        ),
                        if (_hovered)
                          const Positioned(
                            top: 6,
                            right: 6,
                            child: DecoratedBox(
                              decoration: BoxDecoration(color: Colors.black54, shape: BoxShape.circle),
                              child: Padding(
                                padding: EdgeInsets.all(4),
                                child: Icon(Icons.zoom_in, size: 18, color: Colors.white),
                              ),
                            ),
                          ),
                      ],
                    );
                  },
                ),
              ),
            ),
          ),
        ),
      );
}

String fileName(String path) => path.split(RegExp(r'[/\\]')).last;

/// Open a file in the platform's own application for it.
Future<void> openOutsideKineto(String path) async {
  final (executable, arguments) = Platform.isMacOS
      ? ('open', <String>[path])
      : Platform.isWindows
          ? ('cmd', <String>['/c', 'start', '', path])
          : ('xdg-open', <String>[path]);
  await Process.start(executable, arguments, mode: ProcessStartMode.detached);
}

/// Shows [paths] over the whole window from [initial]: the wheel or a pinch
/// zooms where the pointer is, dragging moves, a double click zooms in and
/// back, and the arrow keys step through them.
Future<void> showMediaViewer(BuildContext context, List<String> paths, {int initial = 0}) => showGeneralDialog<void>(
      context: context,
      barrierColor: Colors.black.withValues(alpha: .9),
      transitionDuration: const Duration(milliseconds: 120),
      pageBuilder: (context, animation, secondaryAnimation) => MediaViewer(paths: paths, initial: initial),
    );

class MediaViewer extends StatefulWidget {
  const MediaViewer({required this.paths, this.initial = 0, super.key});

  final List<String> paths;
  final int initial;

  @override
  State<MediaViewer> createState() => _MediaViewerState();
}

class _MediaViewerState extends State<MediaViewer> {
  final TransformationController _view = TransformationController();
  late int _index = widget.initial.clamp(0, widget.paths.length - 1);
  Size _viewport = Size.zero;
  Offset? _doubleTap;

  /// The image's own size in pixels, once it is read.
  Size? _pixels;
  ImageStream? _stream;
  late final ImageStreamListener _listener = ImageStreamListener(
    (info, synchronousCall) {
      final pixels = Size(info.image.width.toDouble(), info.image.height.toDouble());
      info.dispose();
      if (mounted) setState(() => _pixels = pixels);
    },
    onError: (error, stackTrace) {
      if (mounted) setState(() => _pixels = null);
    },
  );

  String get _path => widget.paths[_index];

  @override
  void initState() {
    super.initState();
    _read();
  }

  @override
  void dispose() {
    _stream?.removeListener(_listener);
    _view.dispose();
    super.dispose();
  }

  void _read() {
    _stream?.removeListener(_listener);
    _pixels = null;
    _stream = FileImage(File(_path)).resolve(ImageConfiguration.empty)..addListener(_listener);
  }

  void _step(int by) {
    final next = _index + by;
    if (next < 0 || next >= widget.paths.length) return;
    setState(() {
      _index = next;
      _view.value = Matrix4.identity();
      _read();
    });
  }

  /// The zoom at which one pixel of the image is one pixel of the screen.
  double? get _actual {
    final pixels = _pixels;
    if (pixels == null || _viewport.isEmpty) return null;
    final fitted = math.min(_viewport.width / pixels.width, _viewport.height / pixels.height);
    return 1 / (fitted * MediaQuery.devicePixelRatioOf(context));
  }

  double get _most => math.max(8, 4 * (_actual ?? 1));

  /// Zooms to [scale] of the fitted image, keeping the point under [focal]
  /// (the middle unless given) where it is.
  void _zoom(double scale, [Offset? focal]) {
    final at = focal ?? _viewport.center(Offset.zero);
    final scene = _view.toScene(at);
    final s = scale.clamp(1.0, _most);
    final dx = (at.dx - scene.dx * s).clamp(_viewport.width * (1 - s), 0.0);
    final dy = (at.dy - scene.dy * s).clamp(_viewport.height * (1 - s), 0.0);
    _view.value = Matrix4.diagonal3Values(s, s, 1)..setTranslationRaw(dx, dy, 0);
  }

  double get _scale => _view.value.getMaxScaleOnAxis();

  void _zoomIn() => _zoom(_scale * 1.5);
  void _zoomOut() => _zoom(_scale / 1.5);
  void _fit() => _zoom(1);
  void _oneToOne() => _zoom(math.max(1, _actual ?? 1));

  KeyEventResult _key(FocusNode node, KeyEvent event) {
    if (event is KeyUpEvent) return KeyEventResult.ignored;
    final key = event.logicalKey;
    final character = event.character;
    if (key == LogicalKeyboardKey.escape) {
      Navigator.of(context).pop();
    } else if (key == LogicalKeyboardKey.arrowLeft) {
      _step(-1);
    } else if (key == LogicalKeyboardKey.arrowRight) {
      _step(1);
    } else if (character == '+' || character == '=' || key == LogicalKeyboardKey.numpadAdd) {
      _zoomIn();
    } else if (character == '-' || key == LogicalKeyboardKey.numpadSubtract) {
      _zoomOut();
    } else if (character == '0') {
      _fit();
    } else if (character == '1') {
      _oneToOne();
    } else {
      return KeyEventResult.ignored;
    }
    return KeyEventResult.handled;
  }

  Widget _control(String tip, Object face, VoidCallback? onPressed, String key) => Padding(
        padding: const EdgeInsets.only(left: 6),
        child: Tooltip(
          message: tip,
          child: KinetoButton(
            key: ValueKey<String>('viewer-$key'),
            onPressed: onPressed,
            padding: face is IconData ? const EdgeInsets.all(7) : const EdgeInsets.symmetric(horizontal: 8, vertical: 7),
            child: face is IconData ? Icon(face) : Text('$face', style: const TextStyle(height: 16 / 13.5)),
          ),
        ),
      );

  @override
  Widget build(BuildContext context) {
    const light = Colors.white;
    final many = widget.paths.length > 1;
    final caption = Theme.of(context).textTheme.bodyMedium?.copyWith(color: light);
    return Focus(
      autofocus: true,
      onKeyEvent: _key,
      child: Material(
        type: MaterialType.transparency,
        child: SafeArea(
          child: Column(
            children: <Widget>[
              Padding(
                padding: const EdgeInsets.fromLTRB(16, 10, 16, 10),
                child: Row(
                  children: <Widget>[
                    Expanded(
                      child: SelectionArea(
                        child: ListenableBuilder(
                          listenable: _view,
                          builder: (context, _) {
                            final pixels = _pixels;
                            final actual = _actual;
                            return Text(
                              <String>[
                                fileName(_path),
                                if (many) '${_index + 1} / ${widget.paths.length}',
                                if (pixels != null) '${pixels.width.round()} × ${pixels.height.round()}',
                                if (actual != null) '${(100 * _scale / actual).round()}%',
                              ].join(' · '),
                              key: const ValueKey<String>('viewer-caption'),
                              style: caption,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                            );
                          },
                        ),
                      ),
                    ),
                    if (many) ...<Widget>[
                      _control('Previous (←)', Icons.chevron_left, _index > 0 ? () => _step(-1) : null, 'previous'),
                      _control(
                        'Next (→)',
                        Icons.chevron_right,
                        _index < widget.paths.length - 1 ? () => _step(1) : null,
                        'next',
                      ),
                    ],
                    const SizedBox(width: 10),
                    _control('Zoom out (−)', Icons.zoom_out, _zoomOut, 'zoom-out'),
                    _control('Fit the window (0)', Icons.fit_screen_outlined, _fit, 'fit'),
                    _control('Actual pixels (1)', '1:1', _oneToOne, 'actual'),
                    _control('Zoom in (+)', Icons.zoom_in, _zoomIn, 'zoom-in'),
                    const SizedBox(width: 10),
                    _control('Open outside Kineto', Icons.open_in_new, () => openOutsideKineto(_path), 'open'),
                    _control('Close (Esc)', Icons.close, () => Navigator.of(context).pop(), 'close'),
                  ],
                ),
              ),
              Expanded(
                child: LayoutBuilder(
                  builder: (context, constraints) {
                    if (constraints.biggest != _viewport) {
                      _viewport = constraints.biggest;
                      // The caption above says how far in it is zoomed.
                      WidgetsBinding.instance.addPostFrameCallback((_) {
                        if (mounted) setState(() {});
                      });
                    }
                    return GestureDetector(
                      onDoubleTapDown: (details) => _doubleTap = details.localPosition,
                      onDoubleTap: () => _scale > 1.01 ? _fit() : _zoom(math.max(2.5, _actual ?? 1), _doubleTap),
                      child: InteractiveViewer(
                        key: const ValueKey<String>('viewer-image'),
                        transformationController: _view,
                        minScale: 1,
                        maxScale: _most,
                        child: SizedBox.fromSize(
                          size: _viewport,
                          child: Image.file(
                            File(_path),
                            key: ValueKey<String>(_path),
                            fit: BoxFit.contain,
                            filterQuality: FilterQuality.medium,
                            errorBuilder: (context, error, stackTrace) => Center(
                              child: Text('This image cannot be read', style: caption),
                            ),
                          ),
                        ),
                      ),
                    );
                  },
                ),
              ),
              Padding(
                padding: const EdgeInsets.all(8),
                child: Text(
                  'Scroll or pinch to zoom · drag to move · double-click to zoom in and back'
                  '${many ? ' · ← → for the others' : ''} · Esc to close',
                  style: caption?.copyWith(color: light.withValues(alpha: .6), fontSize: 12),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
