import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/demo_workspace.dart';
import 'package:kineto/kineto_controls.dart';
import 'package:kineto/kineto_layout.dart';
import 'package:kineto/native/kineto_engine.dart';
import 'package:wabisabi/wabisabi.dart';

void main() {
  testWidgets('the sun and moon key switches the WabiSabi palettes and stays small', (tester) async {
    await tester.pumpWidget(const _ThemeHarness());

    expect(WabTheme.isDark, isTrue);
    expect(find.byIcon(Icons.light_mode_outlined), findsOneWidget, reason: 'the sun leads out of the dark');
    expect(find.byIcon(Icons.dark_mode_outlined), findsNothing);
    final size = tester.getSize(find.byKey(const ValueKey<String>('theme-toggle')));
    expect(size.width, lessThan(40), reason: 'the key keeps its size wherever it sits');
    expect(size.height, lessThan(40));

    await tester.tap(find.byKey(const ValueKey<String>('theme-toggle')));
    await tester.pump();

    expect(WabTheme.isDark, isFalse);
    expect(find.byIcon(Icons.dark_mode_outlined), findsOneWidget, reason: 'the moon leads back');
    expect(find.byIcon(Icons.light_mode_outlined), findsNothing);
  });

  testWidgets('buttons are plain blocks of colour, gold for the chosen one', (tester) async {
    await tester.pumpWidget(const _ThemeHarness());

    expect(find.byType(WabButton), findsNothing, reason: 'no textured material');
    expect(find.byType(KinetoButton), findsWidgets);
    final chosen = tester.widget<KinetoButton>(find.byKey(const ValueKey<String>('direction-reaction')));
    expect(chosen.primary, isTrue, reason: 'the chosen direction is gold');
    final other = tester.widget<KinetoButton>(find.byKey(const ValueKey<String>('direction-tension')));
    expect(other.primary, isFalse);
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
      home: Row(
        children: <Widget>[
          KinetoSidebar(
            places: const <KinetoPlace>[KinetoPlace(page: 'shots', label: 'Shots', icon: Icons.movie_outlined)],
            current: 'shots',
            onShow: (_) {},
            isDarkMode: darkMode,
            onThemeToggle: () => setState(() => darkMode = !darkMode),
          ),
          Expanded(
            child: DemoWorkspaceView(
              engineSnapshot: const KinetoEngineSnapshot(abiVersion: 9),
              shotSnapshot: shot,
              shotSnapshots: const <KinetoShotSnapshot>[shot, shot],
              activeShotIndex: 0,
              onShotChanged: (_) {},
              onDirectionChanged: (_) {},
              onGenerate: () {},
              onSelect: (_) {},
              onLock: () {},
              onReset: () {},
            ),
          ),
        ],
      ),
    );
  }
}
