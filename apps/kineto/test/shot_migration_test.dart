import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/demo_workspace.dart';
import 'package:kineto/native/kineto_engine.dart';
import 'package:wabisabi/wabisabi.dart';

void main() {
  const pending = KinetoShotSnapshot(
    direction: KinetoShotDirection.reaction,
    generated: true,
    stale: false,
    selectedIndex: null,
    locked: false,
    candidateCount: 3,
    generation: 1,
    supersededCount: 0,
    migrationPending: true,
  );
  const empty = KinetoShotSnapshot(
    direction: KinetoShotDirection.reaction,
    generated: false,
    stale: false,
    selectedIndex: null,
    locked: false,
    candidateCount: 0,
    generation: 0,
    supersededCount: 0,
  );

  Future<void> pump(
    WidgetTester tester,
    KinetoShotSnapshot shot, {
    VoidCallback? onMigrate,
    VoidCallback? onGenerate,
  }) async {
    tester.view.physicalSize = const Size(1280, 1400);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(
      MaterialApp(
        theme: WabTheme.materialTheme(lightTheme: false),
        home: DemoWorkspaceView(
          engineSnapshot: const KinetoEngineSnapshot(abiVersion: 11),
          shotSnapshot: shot,
          shotSnapshots: <KinetoShotSnapshot>[shot, empty],
          activeShotIndex: 0,
          onShotChanged: (_) {},
          onDirectionChanged: (_) {},
          onGenerate: onGenerate ?? () {},
          onSelect: (_) {},
          onLock: () {},
          onReset: () {},
          onMigrate: onMigrate,
        ),
      ),
    );
  }

  testWidgets('legacy shot offers migration and refuses other changes', (
    tester,
  ) async {
    var migrateCalls = 0;
    var generateCalls = 0;
    await pump(
      tester,
      pending,
      onMigrate: () => migrateCalls++,
      onGenerate: () => generateCalls++,
    );

    expect(find.text('Legacy hashes need migration'), findsOneWidget);
    await tester.tap(find.text('Regenerate'), warnIfMissed: false);
    expect(generateCalls, 0);

    await tester.ensureVisible(find.text('Migrate with backup'));
    await tester.tap(find.text('Migrate with backup'));
    expect(migrateCalls, 1);
  });

  testWidgets('unrecognised hashes explain the read-only shot', (tester) async {
    const invalid = KinetoShotSnapshot(
      direction: KinetoShotDirection.reaction,
      generated: true,
      stale: false,
      selectedIndex: null,
      locked: false,
      candidateCount: 3,
      generation: 1,
      supersededCount: 0,
      hasInvalidHashes: true,
    );
    await pump(tester, invalid);

    expect(find.text('Unrecognised stored hashes'), findsOneWidget);
    expect(find.text('Migrate with backup'), findsNothing);
  });

  test('snapshot flags decode from the native bit layout', () {
    final decoded = KinetoShotSnapshot.fromBits((1 << 3) | (1 << 4));
    expect(decoded.migrationPending, isTrue);
    expect(decoded.hasInvalidHashes, isTrue);
    expect(decoded.changesBlocked, isTrue);
  });
}
