import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/demo_workspace.dart';
import 'package:kineto/native/kineto_engine.dart';
import 'package:wabisabi/wabisabi.dart';

void main() {
  test('recoveries are described for the director', () {
    final notes = describeRecoveries(const <KinetoRecovery>[
      KinetoRecovery(shotIndex: 0, revisionOrCount: 3, errorCode: 0),
      KinetoRecovery(shotIndex: 1, revisionOrCount: 2, errorCode: 121),
      KinetoRecovery(shotIndex: null, revisionOrCount: 2, errorCode: 121),
    ]);

    expect(notes, hasLength(3));
    expect(notes[0], contains('Generation 3 of Shot 001'));
    expect(notes[0], contains('has been completed'));
    expect(notes[1], contains('Generation 2 of Shot 002'));
    expect(notes[1], contains('could not be completed'));
    expect(notes[1], contains('tries again the next time'));
    expect(notes[2], contains('2 job record(s) could not be read'));
  });

  testWidgets('the workspace shows recovered work until dismissed', (tester) async {
    tester.view.physicalSize = const Size(1280, 1400);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    const shot = KinetoShotSnapshot(
      direction: KinetoShotDirection.reaction,
      generated: true,
      stale: false,
      selectedIndex: null,
      locked: false,
      candidateCount: 3,
      generation: 1,
      supersededCount: 0,
    );
    var dismissed = 0;
    await tester.pumpWidget(
      MaterialApp(
        theme: WabTheme.materialTheme(lightTheme: false),
        home: DemoWorkspaceView(
          engineSnapshot: const KinetoEngineSnapshot(abiVersion: 15),
          shotSnapshot: shot,
          shotSnapshots: const <KinetoShotSnapshot>[shot, shot],
          activeShotIndex: 0,
          recoveryNotes: const <String>['Generation 1 of Shot 001 was interrupted and has been completed.'],
          onDismissRecovery: () => dismissed++,
          onShotChanged: (_) {},
          onDirectionChanged: (_) {},
          onGenerate: () {},
          onSelect: (_) {},
          onLock: () {},
          onReset: () {},
        ),
      ),
    );

    expect(find.text('Recovered interrupted work'), findsOneWidget);
    expect(find.textContaining('has been completed'), findsOneWidget);
    await tester.tap(find.text('Dismiss'));
    expect(dismissed, 1);
  });
}
