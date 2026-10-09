import 'dart:async';

import 'package:flutter/material.dart';
import 'package:wabisabi/wabisabi.dart';

import 'native/kineto_engine.dart';

/// A request under way: what it is, where it was asked for, and the button
/// that asked.
@immutable
class KinetoTask {
  const KinetoTask({required this.page, required this.label, required this.started, this.button});

  /// The workspace it was asked for in.
  final String page;

  /// What is being done, such as “Analysing the story with Claude Code”.
  final String label;
  final DateTime started;

  /// The key of the button that asked, which shows the work on itself.
  final Key? button;

  /// The label's first word, for the button: “Analysing”.
  String get short => label.split(' ').first;
}

/// How the last request ended.
@immutable
class KinetoOutcome {
  const KinetoOutcome({required this.task, this.problem, this.retry});

  final KinetoTask task;

  /// Why it failed; `null` when it succeeded.
  final String? problem;

  /// Asks again, offered only when the refusal says asking again is the
  /// director's call, such as a paid request that may have been charged.
  final Future<void> Function()? retry;
}

/// The project's requests, one at a time as its worker takes them. They
/// belong to the project, not to the page that asked: a director who moves
/// to another workspace still sees the work in the status bar and on the
/// button, and finds the result when they come back.
class KinetoWork extends ChangeNotifier {
  KinetoTask? _running;
  KinetoOutcome? _last;
  int _finished = 0;
  Key? _armed;

  KinetoTask? get running => _running;
  KinetoOutcome? get last => _last;
  bool get busy => _running != null;

  /// Counts finished requests; a page reads the project again when it moves.
  int get finished => _finished;

  /// Called by a button around its press, so the request it starts shows on
  /// it.
  void arm(Key? button) => _armed = button;

  /// Run [action] as the project's request, unless one is under way.
  /// Returns whether it succeeded. A refusal with [retryCode] offers
  /// [retry].
  Future<bool> run(
    String page,
    String label,
    Future<void> Function() action, {
    Future<void> Function()? retry,
    int retryCode = KinetoProjectException.paidCallUnresolved,
  }) async {
    if (_running != null) return false;
    final task = KinetoTask(page: page, label: label, started: DateTime.now(), button: _armed);
    _running = task;
    _last = null;
    notifyListeners();
    String? problem;
    Future<void> Function()? offer;
    try {
      await action();
    } on KinetoProjectException catch (failure) {
      // The status says what kind of failure; the reason, when there is
      // one, says what the provider or the engine actually said.
      problem = failure.explanation;
      if (failure.code == retryCode && retry != null) offer = retry;
    } on Object catch (failure) {
      problem = failure.toString();
    }
    _running = null;
    _last = KinetoOutcome(task: task, problem: problem, retry: offer);
    _finished++;
    notifyListeners();
    return problem == null;
  }

  /// Why the last request asked for in [page] failed, if it did.
  String? problemOn(String page) => _last?.task.page == page ? _last?.problem : null;

  /// Asks again for the last request in [page], when that is offered.
  VoidCallback? retryOn(String page) {
    final last = _last;
    if (last == null || last.task.page != page || last.retry == null) return null;
    return () => run(page, last.task.label, last.retry!, retry: last.retry);
  }

  /// Forget how the last request ended.
  void dismiss() {
    if (_last == null) return;
    _last = null;
    notifyListeners();
  }
}

/// Makes the project's [KinetoWork] reachable from any button, dialogs
/// included.
class KinetoWorkScope extends InheritedNotifier<KinetoWork> {
  const KinetoWorkScope({required KinetoWork? work, required super.child, super.key}) : super(notifier: work);

  static KinetoWork? maybeOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<KinetoWorkScope>()?.notifier;
}

/// The strip along the bottom of the window: the request under way and how
/// long it has run, or how the last one ended, whatever workspace is open.
class KinetoStatusBar extends StatefulWidget {
  const KinetoStatusBar({required this.work, this.pageLabel, this.onShow, super.key});

  final KinetoWork? work;

  /// The name a page is shown under, such as “Story & screenplay”.
  final String Function(String page)? pageLabel;

  /// Opens the page a request was asked for in.
  final ValueChanged<String>? onShow;

  @override
  State<KinetoStatusBar> createState() => _KinetoStatusBarState();
}

class _KinetoStatusBarState extends State<KinetoStatusBar> {
  Timer? _clock;

  @override
  void initState() {
    super.initState();
    widget.work?.addListener(_changed);
    _changed();
  }

  @override
  void didUpdateWidget(KinetoStatusBar old) {
    super.didUpdateWidget(old);
    if (old.work != widget.work) {
      old.work?.removeListener(_changed);
      widget.work?.addListener(_changed);
      _changed();
    }
  }

  @override
  void dispose() {
    widget.work?.removeListener(_changed);
    _clock?.cancel();
    super.dispose();
  }

  void _changed() {
    final running = widget.work?.running != null;
    if (running && _clock == null) {
      _clock = Timer.periodic(const Duration(seconds: 1), (_) => setState(() {}));
    } else if (!running) {
      _clock?.cancel();
      _clock = null;
    }
    if (mounted) setState(() {});
  }

  static String _elapsed(Duration duration) {
    final seconds = duration.inSeconds;
    return '${seconds ~/ 60}:${(seconds % 60).toString().padLeft(2, '0')}';
  }

  @override
  Widget build(BuildContext context) {
    // WabiSabi keeps its palette in statics the theme sets; depending on
    // the theme redraws this when the director switches it.
    Theme.of(context);
    final work = widget.work;
    final running = work?.running;
    final last = work?.last;
    final style = TextStyle(color: WabTheme.mutedColor, fontSize: 13, fontFamilyFallback: kWabKaiFallback);
    String where(String page) => widget.pageLabel?.call(page) ?? page;

    final children = <Widget>[];
    if (running != null) {
      children.addAll(<Widget>[
        SizedBox.square(dimension: 12, child: CircularProgressIndicator(strokeWidth: 1.6, color: WabTheme.accentColor)),
        const SizedBox(width: 10),
        Flexible(
          child: KinetoWorkingText(
            running.label,
            style: style.copyWith(color: WabTheme.textColor),
            overflow: TextOverflow.ellipsis,
          ),
        ),
        Text('  ${_elapsed(DateTime.now().difference(running.started))} · ${where(running.page)}', style: style),
        const Spacer(),
        if (widget.onShow != null)
          TextButton(
            key: const ValueKey<String>('status-show'),
            onPressed: () => widget.onShow!(running.page),
            child: Text('Open ${where(running.page)}', style: style.copyWith(color: WabTheme.accentColor)),
          ),
      ]);
    } else if (last != null) {
      final failed = last.problem != null;
      children.addAll(<Widget>[
        Icon(
          failed ? Icons.error_outline : Icons.check,
          size: 14,
          color: failed ? WabTheme.sealColor : WabTheme.onColor,
        ),
        const SizedBox(width: 10),
        Expanded(
          child: Text(
            failed ? '${last.task.label} failed: ${last.problem}' : '${last.task.label}: done',
            style: style.copyWith(color: failed ? WabTheme.textColor : WabTheme.mutedColor),
            overflow: TextOverflow.ellipsis,
          ),
        ),
        if (widget.onShow != null)
          TextButton(
            key: const ValueKey<String>('status-show'),
            onPressed: () => widget.onShow!(last.task.page),
            child: Text('Open ${where(last.task.page)}', style: style.copyWith(color: WabTheme.accentColor)),
          ),
        IconButton(
          key: const ValueKey<String>('status-dismiss'),
          tooltip: 'Dismiss',
          iconSize: 14,
          visualDensity: VisualDensity.compact,
          onPressed: work!.dismiss,
          icon: Icon(Icons.close, color: WabTheme.mutedColor),
        ),
      ]);
    } else {
      children.add(Text('Ready', style: style));
    }
    return Material(
      key: const ValueKey<String>('status-bar'),
      color: WabTheme.surfaceColor,
      child: Container(
        height: 32,
        padding: const EdgeInsets.symmetric(horizontal: 16),
        decoration: BoxDecoration(
          border: Border(top: BorderSide(color: WabTheme.lineColor, width: .6)),
        ),
        child: Row(children: children),
      ),
    );
  }
}

/// [text] followed by three dots that light one after another, so work that
/// takes minutes never looks stuck.
class KinetoWorkingText extends StatefulWidget {
  const KinetoWorkingText(this.text, {this.style, this.overflow, super.key});

  final String text;
  final TextStyle? style;
  final TextOverflow? overflow;

  @override
  State<KinetoWorkingText> createState() => _KinetoWorkingTextState();
}

class _KinetoWorkingTextState extends State<KinetoWorkingText> with SingleTickerProviderStateMixin {
  late final AnimationController _beat = AnimationController(vsync: this, duration: const Duration(milliseconds: 1200))
    ..repeat();

  @override
  void dispose() {
    _beat.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => AnimatedBuilder(
    animation: _beat,
    builder: (context, _) {
      final style = DefaultTextStyle.of(context).style.merge(widget.style);
      final color = style.color ?? WabTheme.mutedColor;
      // Each dot brightens in turn and fades; unlit dots keep their room
      // so the text never shifts.
      TextSpan dot(int index) {
        final phase = (_beat.value * 3 - index) % 3;
        final lit = phase < 1 ? 1 - (phase - .5).abs() * 2 : 0.0;
        return TextSpan(
          text: '.',
          style: TextStyle(color: color.withValues(alpha: .25 + .75 * lit)),
        );
      }

      return Text.rich(
        TextSpan(text: widget.text, children: <TextSpan>[dot(0), dot(1), dot(2)]),
        style: style,
        overflow: widget.overflow,
        maxLines: 1,
      );
    },
  );
}
