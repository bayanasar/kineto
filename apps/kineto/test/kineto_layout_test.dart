import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/kineto_controls.dart';
import 'package:kineto/kineto_layout.dart';
import 'package:kineto/kineto_work.dart';
import 'package:wabisabi/wabisabi.dart';

Future<void> _pump(WidgetTester tester, Size size, Widget child) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  await tester.pumpWidget(MaterialApp(theme: WabTheme.materialTheme(lightTheme: false), home: child));
}

const KinetoWorkspace _workspace = KinetoWorkspace(
  title: 'Shots',
  notices: <Widget>[Text('A refusal', key: ValueKey<String>('notice'))],
  side: <Widget>[SizedBox(height: 40, child: Text('The shots', key: ValueKey<String>('side')))],
  main: <Widget>[SizedBox(height: 40, child: Text('The open shot', key: ValueKey<String>('main')))],
);

void main() {
  testWidgets('a wide window shows a workspace in two columns side by side', (tester) async {
    await _pump(tester, const Size(1600, 1000), _workspace);
    expect(find.text('Shots'), findsOneWidget);
    final side = tester.getTopLeft(find.byKey(const ValueKey<String>('side')));
    final main = tester.getTopLeft(find.byKey(const ValueKey<String>('main')));
    final notice = tester.getTopLeft(find.byKey(const ValueKey<String>('notice')));
    expect(main.dx, greaterThan(side.dx + 300), reason: 'the work sits beside what it is picked from');
    expect(main.dy, side.dy);
    expect(notice.dy, lessThan(side.dy), reason: 'a notice runs across both');
  });

  testWidgets('a narrow window stacks the columns', (tester) async {
    await _pump(tester, const Size(800, 1000), _workspace);
    final side = tester.getTopLeft(find.byKey(const ValueKey<String>('side')));
    final main = tester.getTopLeft(find.byKey(const ValueKey<String>('main')));
    expect(main.dx, side.dx);
    expect(main.dy, greaterThan(side.dy));
  });

  testWidgets('the sidebar marks the open workspace and the ones with work under way', (tester) async {
    final work = KinetoWork();
    addTearDown(work.dispose);
    final shown = <String>[];
    final answer = Completer<void>();
    await _pump(
      tester,
      const Size(1600, 1000),
      KinetoWorkScope(
        work: work,
        child: Row(
          children: <Widget>[
            KinetoSidebar(
              places: const <KinetoPlace>[
                KinetoPlace(page: 'production', label: 'Bible, style & cast', icon: Icons.face),
                KinetoPlace(page: 'shots', label: 'Shots', icon: Icons.movie),
              ],
              current: 'shots',
              onShow: shown.add,
              projectTitle: 'The Bench',
              onThemeToggle: () {},
            ),
            const Expanded(child: SizedBox()),
          ],
        ),
      ),
    );
    expect(find.text('The Bench'), findsOneWidget);
    expect(tester.widget<KinetoButton>(find.byKey(const ValueKey<String>('workspace-shots'))).primary, isTrue);
    expect(tester.widget<KinetoButton>(find.byKey(const ValueKey<String>('workspace-production'))).primary, isFalse);
    expect(find.byKey(const ValueKey<String>('working-production')), findsNothing);

    unawaited(work.run('production', 'Casting', () => answer.future, target: 'character:mira'));
    await tester.pump();
    expect(find.byKey(const ValueKey<String>('working-production')), findsOneWidget);
    expect(find.byKey(const ValueKey<String>('working-shots')), findsNothing);

    await tester.tap(find.byKey(const ValueKey<String>('workspace-production')));
    expect(shown, <String>['production']);
    answer.complete();
    await tester.pump();
    expect(find.byKey(const ValueKey<String>('working-production')), findsNothing);
  });
}
