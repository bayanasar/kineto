import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/agent_desk.dart';
import 'package:kineto/native/kineto_engine.dart';
import 'package:wabisabi/wabisabi.dart';

void main() {
  testWidgets('what agents ask waits at the bottom of the window until the director answers', (tester) async {
    WidgetController.hitTestWarningShouldBeFatal = true;
    var asks = <KinetoAgentAsk>[
      const KinetoAgentAsk(id: 1, connect: true, agent: 'ChatGPT', what: 'connect from the internet, returning to chatgpt.com'),
      const KinetoAgentAsk(id: 2, connect: false, agent: 'Codex (on this computer)', what: 'make keyframes of office/office_02', cost: 'about \$0.12'),
    ];
    final decided = <String>[];
    await tester.pumpWidget(
      MaterialApp(
        theme: WabTheme.materialTheme(lightTheme: false),
        home: Scaffold(
          body: Column(
            children: <Widget>[
              const Expanded(child: SizedBox()),
              KinetoAgentDesk(
                asks: () => asks,
                decide: (id, allow) {
                  if (id == 3) throw const KinetoProjectException(101, 'no longer waits');
                  decided.add('$id ${allow ? 'allow' : 'decline'}');
                  asks = asks.where((ask) => ask.id != id).toList();
                },
                every: const Duration(milliseconds: 100),
              ),
            ],
          ),
        ),
      ),
    );
    expect(find.text('ChatGPT asks to connect from the internet, returning to chatgpt.com.'), findsOneWidget);
    expect(
      find.text('Codex (on this computer) asks to make keyframes of office/office_02 (about \$0.12).'),
      findsOneWidget,
    );

    await tester.tap(find.byKey(const ValueKey<String>('ask-1-allow')));
    await tester.pump();
    await tester.tap(find.byKey(const ValueKey<String>('ask-2-decline')));
    await tester.pump();
    expect(decided, <String>['1 allow', '2 decline']);
    expect(find.byKey(const ValueKey<String>('agent-desk')), findsNothing, reason: 'nothing waits');

    // A request that arrives while the window is open shows on the next read.
    asks = <KinetoAgentAsk>[const KinetoAgentAsk(id: 3, connect: false, agent: 'Claude', what: 'make a video of a/b')];
    await tester.pump(const Duration(milliseconds: 150));
    expect(find.byKey(const ValueKey<String>('ask-3')), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey<String>('ask-3-allow')));
    await tester.pump();
    expect(find.byKey(const ValueKey<String>('agent-desk-problem')), findsOneWidget);
  });
}
