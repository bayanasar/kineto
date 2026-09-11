import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/demo_workspace.dart';
import 'package:kineto/native/kineto_engine.dart';
import 'package:wabisabi/wabisabi.dart';

void main() {
  testWidgets('demo workspace renders first shot generation and scene progress', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(1280, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    var requestedShot = -1;
    const shotOne = KinetoShotSnapshot(
      direction: KinetoShotDirection.intimacy,
      generated: true,
      stale: false,
      selectedIndex: 1,
      locked: false,
      candidateCount: 3,
      generation: 2,
      supersededCount: 3,
    );
    const shotTwo = KinetoShotSnapshot(
      direction: KinetoShotDirection.reaction,
      generated: false,
      stale: false,
      selectedIndex: null,
      locked: false,
      candidateCount: 0,
      generation: 0,
      supersededCount: 0,
    );

    await tester.pumpWidget(
      MaterialApp(
        theme: WabTheme.materialTheme(lightTheme: false),
        home: DemoWorkspaceView(
          engineSnapshot: const KinetoEngineSnapshot(abiVersion: 9),
          shotSnapshot: shotOne,
          shotSnapshots: const <KinetoShotSnapshot>[shotOne, shotTwo],
          activeShotIndex: 0,
          onShotChanged: (index) => requestedShot = index,
          onDirectionChanged: (_) {},
          onGenerate: () {},
          onSelect: (_) {},
          onLock: () {},
          onReset: () {},
        ),
      ),
    );

    expect(find.text('Canonical demo project'), findsOneWidget);
    expect(find.text('Scene 001'), findsOneWidget);
    expect(find.text('In progress'), findsOneWidget);
    expect(find.text('Selected'), findsNWidgets(2));
    expect(find.text('Empty'), findsOneWidget);
    expect(find.text('Shot 001'), findsOneWidget);
    expect(find.text('Shot 002'), findsOneWidget);
    expect(find.text('Scene 001 · Shot 001'), findsOneWidget);
    expect(find.text('Wide master'), findsOneWidget);
    expect(find.text('Profile medium'), findsOneWidget);
    expect(find.text('Close-up'), findsOneWidget);
    expect(find.text('Generation 2 · Candidates'), findsOneWidget);
    expect(find.textContaining('3 superseded'), findsOneWidget);
    expect(find.text('Native ABI 9'), findsOneWidget);

    await tester.ensureVisible(find.text('Shot 002'));
    await tester.tap(find.text('Shot 002'));
    expect(requestedShot, 1);
  });

  testWidgets(
    'fully locked scene reports ready while second shot stays independently visible',
    (tester) async {
      const shotOne = KinetoShotSnapshot(
        direction: KinetoShotDirection.reaction,
        generated: true,
        stale: false,
        selectedIndex: 0,
        locked: true,
        candidateCount: 3,
        generation: 1,
        supersededCount: 0,
      );
      const shotTwo = KinetoShotSnapshot(
        direction: KinetoShotDirection.tension,
        generated: true,
        stale: false,
        selectedIndex: 2,
        locked: true,
        candidateCount: 3,
        generation: 1,
        supersededCount: 0,
      );

      await tester.pumpWidget(
        MaterialApp(
          theme: WabTheme.materialTheme(lightTheme: false),
          home: DemoWorkspaceView(
            engineSnapshot: const KinetoEngineSnapshot(abiVersion: 9),
            shotSnapshot: shotTwo,
            shotSnapshots: const <KinetoShotSnapshot>[shotOne, shotTwo],
            activeShotIndex: 1,
            onShotChanged: (_) {},
            onDirectionChanged: (_) {},
            onGenerate: () {},
            onSelect: (_) {},
            onLock: () {},
            onReset: () {},
          ),
        ),
      );

      expect(find.text('Ready'), findsOneWidget);
      expect(find.text('Locked'), findsNWidgets(3));
      expect(find.text('Scene 001 · Shot 002'), findsOneWidget);
      expect(find.text('Doorway two-shot'), findsOneWidget);
      expect(find.text('Over shoulder'), findsOneWidget);
      expect(find.text('Exit detail'), findsOneWidget);
      expect(
        find.text("Direction is frozen with this shot's locked selection."),
        findsOneWidget,
      );
      expect(find.text('Selection locked'), findsOneWidget);
      expect(find.text('Reset shot'), findsOneWidget);
    },
  );

  testWidgets('stale canonical candidates remain visible but cannot be locked', (
    tester,
  ) async {
    var lockCalls = 0;
    const staleShot = KinetoShotSnapshot(
      direction: KinetoShotDirection.tension,
      generated: true,
      stale: true,
      selectedIndex: 1,
      locked: false,
      candidateCount: 3,
      generation: 1,
      supersededCount: 0,
    );
    const emptyShot = KinetoShotSnapshot(
      direction: KinetoShotDirection.reaction,
      generated: false,
      stale: false,
      selectedIndex: null,
      locked: false,
      candidateCount: 0,
      generation: 0,
      supersededCount: 0,
    );

    await tester.pumpWidget(
      MaterialApp(
        theme: WabTheme.materialTheme(lightTheme: false),
        home: DemoWorkspaceView(
          engineSnapshot: const KinetoEngineSnapshot(abiVersion: 9),
          shotSnapshot: staleShot,
          shotSnapshots: const <KinetoShotSnapshot>[staleShot, emptyShot],
          activeShotIndex: 0,
          onShotChanged: (_) {},
          onDirectionChanged: (_) {},
          onGenerate: () {},
          onSelect: (_) {},
          onLock: () => lockCalls++,
          onReset: () {},
        ),
      ),
    );

    expect(find.text('Stale'), findsOneWidget);
    expect(find.textContaining('stale after the direction changed'), findsOneWidget);
    expect(find.text('Regenerate before locking'), findsOneWidget);
    expect(find.text('Profile medium'), findsOneWidget);

    await tester.tap(find.text('Regenerate before locking'), warnIfMissed: false);
    expect(lockCalls, 0);
  });
}
