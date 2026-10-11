import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:wabisabi/wabisabi.dart';

import 'kineto_controls.dart';
import 'native/kineto_engine.dart';

void _decide(int id, bool allow) => decideKinetoAgentAsk(id, allow: allow);

/// What agents outside Kineto ask that waits for the director (ADR-0011): a
/// command that pays a service per call, or an agent connecting from the
/// internet. Shown above the status bar whatever workspace is open, until
/// the director allows or declines it.
class KinetoAgentDesk extends StatefulWidget {
  const KinetoAgentDesk({
    this.asks = kinetoAgentAsks,
    this.decide = _decide,
    this.every = const Duration(seconds: 1),
    super.key,
  });

  /// What waits, read every [every].
  final List<KinetoAgentAsk> Function() asks;
  final void Function(int id, bool allow) decide;
  final Duration every;

  @override
  State<KinetoAgentDesk> createState() => _KinetoAgentDeskState();
}

class _KinetoAgentDeskState extends State<KinetoAgentDesk> {
  List<KinetoAgentAsk> _asks = const <KinetoAgentAsk>[];
  String? _problem;
  Timer? _timer;

  @override
  void initState() {
    super.initState();
    _read();
    _timer = Timer.periodic(widget.every, (_) => _read());
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  void _read() {
    List<KinetoAgentAsk> asks;
    try {
      asks = widget.asks();
    } on Object {
      // Read again on the next tick.
      return;
    }
    if (mounted && !listEquals(asks, _asks)) setState(() => _asks = asks);
  }

  void _answer(KinetoAgentAsk ask, {required bool allow}) {
    String? problem;
    try {
      widget.decide(ask.id, allow);
    } on KinetoProjectException {
      problem = 'That request no longer waits: the agent gave up, or Kineto closed the project.';
    } on Object catch (error) {
      problem = error.toString();
    }
    setState(() => _problem = problem);
    _read();
  }

  static String _sentence(KinetoAgentAsk ask) {
    final cost = ask.cost == null ? '' : ' (${ask.cost})';
    return '${ask.agent} asks to ${ask.what}$cost.';
  }

  @override
  Widget build(BuildContext context) {
    Theme.of(context);
    if (_asks.isEmpty && _problem == null) return const SizedBox.shrink();
    final style = TextStyle(color: WabTheme.textColor, fontSize: 13, fontFamilyFallback: kWabKaiFallback);
    return Material(
      key: const ValueKey<String>('agent-desk'),
      color: WabTheme.surfaceColor,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 6),
        decoration: BoxDecoration(
          border: Border(top: BorderSide(color: WabTheme.lineColor, width: .6)),
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: <Widget>[
            if (_problem != null)
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 4),
                child: Text(_problem!, key: const ValueKey<String>('agent-desk-problem'), style: style),
              ),
            for (final ask in _asks)
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 4),
                child: Row(
                  children: <Widget>[
                    Icon(
                      ask.connect ? Icons.link : Icons.payments_outlined,
                      size: 16,
                      color: WabTheme.accentColor,
                    ),
                    const SizedBox(width: 10),
                    Expanded(
                      child: Text(_sentence(ask), key: ValueKey<String>('ask-${ask.id}'), style: style),
                    ),
                    const SizedBox(width: 8),
                    KinetoButton(
                      key: ValueKey<String>('ask-${ask.id}-decline'),
                      onPressed: () => _answer(ask, allow: false),
                      child: const Text('Decline'),
                    ),
                    const SizedBox(width: 8),
                    KinetoButton(
                      key: ValueKey<String>('ask-${ask.id}-allow'),
                      primary: true,
                      onPressed: () => _answer(ask, allow: true),
                      child: const Text('Allow'),
                    ),
                  ],
                ),
              ),
          ],
        ),
      ),
    );
  }
}
