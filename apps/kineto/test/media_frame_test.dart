import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/media_frame.dart';
import 'package:wabisabi/wabisabi.dart';

void main() {
  late Directory sandbox;
  setUp(() => sandbox = Directory.systemTemp.createTempSync('kineto-media-frame-'));
  tearDown(() => sandbox.deleteSync(recursive: true));

  Widget frames(List<String> paths) => MaterialApp(
        theme: WabTheme.materialTheme(lightTheme: false),
        home: Scaffold(
          body: Row(
            children: <Widget>[
              for (final path in paths)
                SizedBox(
                  width: 160,
                  child: MediaFrame(key: ValueKey<String>(path), path: path, aspectRatio: 16 / 9, gallery: paths),
                ),
            ],
          ),
        ),
      );

  String caption(WidgetTester tester) => tester.widget<Text>(find.byKey(const ValueKey<String>('viewer-caption'))).data!;
  double scale(WidgetTester tester) => tester
      .widget<InteractiveViewer>(find.byKey(const ValueKey<String>('viewer-image')))
      .transformationController!
      .value
      .getMaxScaleOnAxis();

  testWidgets('a thumbnail opens to zoom in, step through its neighbours and close', (tester) async {
    final paths = <String>['${sandbox.path}/a.png', '${sandbox.path}/b.png', '${sandbox.path}/c.png'];
    await tester.pumpWidget(frames(paths));
    await tester.tap(find.byKey(ValueKey<String>(paths[1])));
    await tester.pumpAndSettle();
    expect(caption(tester), 'b.png · 2 / 3');

    await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
    await tester.pump();
    expect(caption(tester), 'c.png · 3 / 3');
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
    await tester.pump();
    expect(caption(tester), 'c.png · 3 / 3', reason: 'the last stays the last');
    await tester.tap(find.byKey(const ValueKey<String>('viewer-previous')));
    await tester.pump();
    expect(caption(tester), 'b.png · 2 / 3');

    // The wheel zooms in under the pointer and back out.
    final wheel = TestPointer(1, PointerDeviceKind.mouse);
    final middle = tester.getCenter(find.byKey(const ValueKey<String>('viewer-image')));
    await tester.sendEventToBinding(wheel.hover(middle));
    await tester.sendEventToBinding(wheel.scroll(const Offset(0, -100)));
    await tester.pump();
    expect(scale(tester), greaterThan(1.5));
    await tester.sendEventToBinding(wheel.scroll(const Offset(0, 400)));
    await tester.pump();
    expect(scale(tester), 1, reason: 'never smaller than the window');

    await tester.tap(find.byKey(const ValueKey<String>('viewer-zoom-in')));
    await tester.pump();
    expect(scale(tester), closeTo(1.5, 1e-9));
    await tester.sendKeyEvent(LogicalKeyboardKey.digit0);
    await tester.pump();
    expect(scale(tester), 1);

    // A double click zooms in where it lands, and a second one goes back.
    final image = find.byKey(const ValueKey<String>('viewer-image'));
    final spot = tester.getTopLeft(image) + const Offset(100, 80);
    await tester.tapAt(spot);
    await tester.pump(const Duration(milliseconds: 50));
    await tester.tapAt(spot);
    await tester.pumpAndSettle();
    expect(scale(tester), closeTo(2.5, 1e-9));
    final view = tester.widget<InteractiveViewer>(image).transformationController!;
    final local = spot - tester.getTopLeft(image);
    expect(view.toScene(local).dx, closeTo(local.dx, 1e-6), reason: 'the point under the pointer stays put');
    await tester.tapAt(spot);
    await tester.pump(const Duration(milliseconds: 50));
    await tester.tapAt(spot);
    await tester.pumpAndSettle();
    expect(scale(tester), 1);

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey<String>('viewer-caption')), findsNothing);
  });

  testWidgets('actual pixels shows one image pixel on one screen pixel', (tester) async {
    tester.view
      ..physicalSize = const Size(1200, 800)
      ..devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    final path = '${sandbox.path}/wide.png';
    await tester.runAsync(() async {
      final recorder = ui.PictureRecorder();
      Canvas(recorder).drawRect(const Rect.fromLTWH(0, 0, 2400, 1200), Paint()..color = const Color(0xff336699));
      final picture = await recorder.endRecording().toImage(2400, 1200);
      final png = await picture.toByteData(format: ui.ImageByteFormat.png);
      File(path).writeAsBytesSync(png!.buffer.asUint8List());
    });

    await tester.pumpWidget(frames(<String>[path]));
    await tester.tap(find.byKey(ValueKey<String>(path)));
    await tester.pumpAndSettle();
    for (var tries = 0; tries < 100 && !caption(tester).contains('×'); tries++) {
      await tester.runAsync(() => Future<void>.delayed(const Duration(milliseconds: 20)));
      await tester.pump();
    }
    await tester.pump();
    expect(caption(tester), startsWith('wide.png · 2400 × 1200 · '));
    expect(caption(tester), isNot(endsWith(' 100%')), reason: 'it opens fitted to the window');

    await tester.sendKeyEvent(LogicalKeyboardKey.digit1);
    await tester.pump();
    expect(caption(tester), 'wide.png · 2400 × 1200 · 100%');
    final viewer = tester.getSize(find.byKey(const ValueKey<String>('viewer-image')));
    expect(scale(tester), closeTo(2400 / viewer.width, 1e-6));
  });
}
