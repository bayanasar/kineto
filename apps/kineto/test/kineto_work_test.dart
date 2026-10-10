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

  testWidgets('requests for different things run at once, and one that waits says so', (tester) async {
    final work = KinetoWork();
    addTearDown(work.dispose);
    final first = Completer<void>();
    final second = Completer<void>();
    final started = Completer<void>();

    Widget button(String key, String label, Future<void> Function() action, {String? target}) => KinetoButton(
          key: ValueKey<String>(key),
          onPressed: () => work.run('shots', label, action, target: target),
          child: Text(key),
        );
    await tester.pumpWidget(
      MaterialApp(
        theme: WabTheme.materialTheme(lightTheme: false),
        home: KinetoWorkScope(
          work: work,
          child: Scaffold(
            body: Column(
              children: <Widget>[
                button('one', 'Generating shot one', () => first.future, target: 'shot:0'),
                button('two', 'Generating shot two', () => second.future, target: 'shot:1'),
                button('again', 'Selecting', () async {
                  // The engine holds it behind shot one's generation.
                  reportWaiting(true);
                  await started.future;
                  reportWaiting(false);
                  await first.future;
                }, target: 'shot:0'),
              ],
            ),
            bottomNavigationBar: SizedBox(width: 1600, child: KinetoStatusBar(work: work)),
          ),
        ),
      ),
    );
    await tester.tap(find.byKey(const ValueKey<String>('one')));
    await tester.tap(find.byKey(const ValueKey<String>('two')));
    await tester.tap(find.byKey(const ValueKey<String>('again')));
    await tester.pump();

    bool shows(String text) =>
        find.byWidgetPredicate((widget) => widget is KinetoWorkingText && widget.text == text).evaluate().isNotEmpty;
    expect(work.running, hasLength(3));
    expect(shows('Generating shot one') && shows('Generating shot two'), isTrue, reason: 'both in the status bar');
    expect(work.busyWith('shot:0') && work.busyWith('shot:1'), isTrue);
    expect(shows('Waiting'), isTrue, reason: 'the button that waits says so');
    expect(shows('Waiting · Selecting'), isTrue);

    started.complete();
    await tester.pump();
    expect(shows('Waiting'), isFalse);
    expect(shows('Selecting'), isTrue);

    second.complete();
    await tester.pump();
    expect(work.busyWith('shot:1'), isFalse);
    expect(work.busyWith('shot:0'), isTrue, reason: 'shot one is still under way');
    first.complete();
    await tester.pump();
    await tester.pump();
    expect(work.busy, isFalse);
    expect(work.finished, 3);
  });

  testWidgets("a button shows only its own shot's request", (tester) async {
    final work = KinetoWork();
    addTearDown(work.dispose);
    final answer = Completer<void>();
    Widget app(String target) => MaterialApp(
          theme: WabTheme.materialTheme(lightTheme: false),
          home: KinetoWorkScope(
            work: work,
            child: KinetoTargetScope(
              target: target,
              child: Scaffold(
                body: KinetoButton(
                  key: const ValueKey<String>('generate'),
                  onPressed: () => work.run('shots', 'Generating', () => answer.future, target: target),
                  child: const Text('Generate'),
                ),
              ),
            ),
          ),
        );
    await tester.pumpWidget(app('shot:0'));
    await tester.tap(find.byKey(const ValueKey<String>('generate')));
    await tester.pump();
    expect(find.text('Generate'), findsNothing);
    // Another shot's identical button is free.
    await tester.pumpWidget(app('shot:1'));
    expect(find.text('Generate'), findsOneWidget);
    answer.complete();
    await tester.pump();
  });
}
