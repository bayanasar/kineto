import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/demo_workspace.dart';
import 'package:kineto/native/kineto_engine.dart';
import 'package:wabisabi/wabisabi.dart';

void main() {
  testWidgets('workspace toggles WabiSabi dark and light palettes', (tester) async {
    await tester.pumpWidget(const _ThemeHarness());

    expect(WabTheme.isDark, isTrue);
    var toggle = tester.widget<WabToggleButton>(find.byType(WabToggleButton));
    expect(toggle.isOn, isTrue);
    expect(toggle.pair, WabTogglePair.sealZhuwen);
    expect(
      tester.widgetList<WabButton>(find.byType(WabButton)).any(
            (button) => button.kind == WabMaterialKind.zhuwen,
          ),
      isTrue,
    );

    await tester.tap(find.text('Dark mode'));
    await tester.pump();

    expect(WabTheme.isDark, isFalse);
    toggle = tester.widget<WabToggleButton>(find.byType(WabToggleButton));
    expect(toggle.isOn, isFalse);
  });
}

class _ThemeHarness extends StatefulWidget {
  const _ThemeHarness();

  @override
  State<_ThemeHarness> createState() => _ThemeHarnessState();
}

class _ThemeHarnessState extends State<_ThemeHarness> {
  bool darkMode = true;

  static const shot = KinetoShotSnapshot(
    direction: KinetoShotDirection.reaction,
    generated: false,
    stale: false,
    selectedIndex: null,
    locked: false,
    candidateCount: 0,
    generation: 0,
    supersededCount: 0,
  );

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      theme: WabTheme.materialTheme(lightTheme: !darkMode),
      home: DemoWorkspaceView(
        engineSnapshot: const KinetoEngineSnapshot(abiVersion: 9),
        shotSnapshot: shot,
        shotSnapshots: const <KinetoShotSnapshot>[shot, shot],
        activeShotIndex: 0,
        isDarkMode: darkMode,
        onThemeToggle: () => setState(() => darkMode = !darkMode),
        onShotChanged: (_) {},
        onDirectionChanged: (_) {},
        onGenerate: () {},
        onSelect: (_) {},
        onLock: () {},
        onReset: () {},
      ),
    );
  }
}
