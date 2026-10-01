import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/demo_workspace.dart';
import 'package:kineto/native/kineto_engine.dart';
import 'package:wabisabi/wabisabi.dart';

void main() {
  testWidgets('a running native command holds every change until it answers', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(1280, 1400);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    const shot = KinetoShotSnapshot(
      direction: KinetoShotDirection.reaction,
      generated: true,
      stale: false,
      selectedIndex: 0,
      locked: false,
      candidateCount: 3,
      generation: 1,
      supersededCount: 0,
    );
    var calls = 0;
    await tester.pumpWidget(
      MaterialApp(
        theme: WabTheme.materialTheme(lightTheme: false),
        home: DemoWorkspaceView(
          engineSnapshot: const KinetoEngineSnapshot(abiVersion: 13),
          shotSnapshot: shot,
          shotSnapshots: const <KinetoShotSnapshot>[shot, shot],
          activeShotIndex: 0,
          working: true,
          onShotChanged: (_) {},
          onDirectionChanged: (_) => calls++,
          onGenerate: () => calls++,
          onSelect: (_) => calls++,
          onLock: () => calls++,
          onReset: () => calls++,
        ),
      ),
    );

    expect(find.text('Working'), findsOneWidget);
    for (final label in <String>[
      'Regenerate',
      'Lock selection',
      'Reset shot',
      'Tension',
    ]) {
      await tester.ensureVisible(find.text(label));
      await tester.tap(find.text(label), warnIfMissed: false);
    }
    expect(calls, 0);
  });
}
