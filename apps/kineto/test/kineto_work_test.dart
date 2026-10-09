import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/kineto_controls.dart';
import 'package:kineto/kineto_work.dart';
import 'package:kineto/native/kineto_engine.dart';
import 'package:wabisabi/wabisabi.dart';

void main() {
  testWidgets('a request shows on its button and in the status bar, and outlives the page', (tester) async {
    final work = KinetoWork();
    addTearDown(work.dispose);
    final answer = Completer<void>();
    final shown = <String>[];

    Widget app({required bool storyOpen}) => MaterialApp(
          theme: WabTheme.materialTheme(lightTheme: false),
          home: KinetoWorkScope(
            work: work,
            child: Scaffold(
              body: storyOpen
                  ? Center(
                      child: KinetoButton(
                        key: const ValueKey<String>('analyse'),
                        onPressed: () => work.run('screenplay', 'Analysing the story with Claude Code', () => answer.future),
                        child: const Text('Analyse story'),
                      ),
                    )
                  : const Center(child: Text('Another workspace')),
              bottomNavigationBar: KinetoStatusBar(work: work, onShow: shown.add),
            ),
          ),
        );

    await tester.pumpWidget(app(storyOpen: true));
    expect(find.text('Ready'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey<String>('analyse')));
    await tester.pump();

    KinetoWorkingText working(String text) =>
        tester.widget<KinetoWorkingText>(find.byWidgetPredicate((widget) => widget is KinetoWorkingText && widget.text == text));
    expect(working('Analysing'), isNotNull, reason: 'the button says what it is doing');
    expect(working('Analysing the story with Claude Code'), isNotNull, reason: 'so does the status bar');
    expect(find.text('Analyse story'), findsNothing);

    // The director looks at another workspace and comes back: the request
    // is still under way, on a button that was built anew.
    await tester.pumpWidget(app(storyOpen: false));
    await tester.pump();
    expect(working('Analysing the story with Claude Code'), isNotNull);
    await tester.pumpWidget(app(storyOpen: true));
    await tester.pump();
    expect(working('Analysing'), isNotNull);
    expect(tester.widget<KinetoButton>(find.byKey(const ValueKey<String>('analyse'))).onPressed, isNotNull);
    await tester.tap(find.byKey(const ValueKey<String>('analyse')));
    await tester.pump();
    expect(work.finished, 0, reason: 'a busy button asks for nothing more');

    answer.complete();
    await tester.pump();
    await tester.pump();
    expect(find.text('Analyse story'), findsOneWidget);
    expect(find.text('Analysing the story with Claude Code: done'), findsOneWidget);
    expect(work.finished, 1);
    await tester.tap(find.byKey(const ValueKey<String>('status-dismiss')));
    await tester.pump();
    expect(find.text('Ready'), findsOneWidget);
  });

  testWidgets('a refusal stays with the page that asked, with its retry', (tester) async {
    final work = KinetoWork();
    addTearDown(work.dispose);
    var retried = 0;
    await work.run(
      'production',
      'Casting',
      () async => throw const KinetoProjectException(KinetoProjectException.paidCallUnresolved, 'may have been charged'),
      retry: () async => retried++,
    );
    expect(work.problemOn('production'), 'may have been charged');
    expect(work.problemOn('screenplay'), isNull);
    expect(work.retryOn('screenplay'), isNull);
    work.retryOn('production')!();
    await tester.pump();
    expect(retried, 1);
    expect(work.problemOn('production'), isNull);
  });
}
