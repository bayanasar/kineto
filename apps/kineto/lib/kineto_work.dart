import 'dart:async';

import 'package:flutter/material.dart';
import 'package:wabisabi/wabisabi.dart';

import 'native/kineto_engine.dart';

/// A request under way: what it is, where it was asked for, what it works
/// on, and the button that asked.
@immutable
class KinetoTask {
  const KinetoTask({required this.page, required this.label, required this.started, this.button, this.target});

  /// The workspace it was asked for in.
  final String page;

  /// What is being done, such as “Analysing the story with Claude Code”.
  final String label;
  final DateTime started;

  /// The key of the button that asked, which shows the work on itself.
  final Key? button;

  /// What it works on, such as `shot:2` or `character:mira`: controls for
  /// that wait while it runs. Requests for different things run at once.
  final String? target;

  /// The label's first word, for the button: “Analysing”.
  String get short => label.split(' ').first;
}

/// How a request ended.
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

/// The project's requests. Several run at once: the engine runs a few that
/// wait on a provider together, and those about the same thing one after
/// another, so a request may wait before it starts. They belong to the
/// project, not to the page that asked: a director who moves to another
/// workspace still sees the work in the status bar and on the button, and
/// finds the result when they come back.
class KinetoWork extends ChangeNotifier {
  final List<KinetoTask> _running = <KinetoTask>[];
  final Set<KinetoTask> _waiting = <KinetoTask>{};
  /// How requests ended, by page and then by target, each page's in the
  /// order they ended: requests for different targets of one page end in any
  /// order, and one never hides another's failure.
  final Map<String, Map<String?, KinetoOutcome>> _outcomes = <String, Map<String?, KinetoOutcome>>{};
  KinetoOutcome? _last;
  int _finished = 0;
  Key? _armed;

  /// The requests under way, oldest first, waiting ones included.
  List<KinetoTask> get running => List<KinetoTask>.unmodifiable(_running);

  /// How the most recent request ended, until it is dismissed.
  KinetoOutcome? get last => _last;
  bool get busy => _running.isNotEmpty;

  /// Counts finished requests; a page reads the project again when it moves.
  int get finished => _finished;

  /// Whether [task] waits to start.
  bool isWaiting(KinetoTask task) => _waiting.contains(task);

  /// The request under way that the button keyed [button] asked for; with
  /// [target], only one about it, since the same button of another shot
  /// may have asked.
  KinetoTask? startedBy(Key? button, {String? target}) {
    if (button == null) return null;
    for (final task in _running.reversed) {
      if (task.button == button && (target == null || task.target == null || task.target == target)) return task;
    }
    return null;
  }

  /// What the requests under way work on.
  Set<String> get targets => <String>{
        for (final task in _running)
          if (task.target != null) task.target!,
      };

  /// Whether a request for [target] is under way.
  bool busyWith(String target) => _running.any((task) => task.target == target);

  /// Whether a request asked for in [page] is under way.
  bool busyOn(String page) => _running.any((task) => task.page == page);

  /// The newest request under way asked for in [page].
  KinetoTask? latestOn(String page) {
    for (final task in _running.reversed) {
      if (task.page == page) return task;
    }
    return null;
  }

  /// Called by a button around its press, so the request it starts shows on
  /// it.
  void arm(Key? button) => _armed = button;

  /// Run [action] as one of the project's requests, about [target] when
  /// given. Returns whether it succeeded. A refusal with [retryCode] offers
  /// [retry].
  Future<bool> run(
    String page,
    String label,
    Future<void> Function() action, {
    String? target,
    Future<void> Function()? retry,
    int retryCode = KinetoProjectException.paidCallUnresolved,
  }) async {
    final task = KinetoTask(page: page, label: label, started: DateTime.now(), button: _armed, target: target);
    _armed = null;
    _running.add(task);
    _outcomes[page]?.remove(target);
    if (_last?.task.page == page && _last?.task.target == target) _last = null;
    notifyListeners();
    String? problem;
    Future<void> Function()? offer;
    try {
      await runReportingWaits((waiting) {
        if (waiting ? _waiting.add(task) : _waiting.remove(task)) notifyListeners();
      }, action);
    } on KinetoProjectException catch (failure) {
      // The status says what kind of failure; the reason, when there is
      // one, says what the provider or the engine actually said.
      problem = failure.explanation;
      if (failure.code == retryCode && retry != null) offer = retry;
    } on Object catch (failure) {
      problem = failure.toString();
    }
    _running.remove(task);
    _waiting.remove(task);
    final outcome = KinetoOutcome(task: task, problem: problem, retry: offer);
    final outcomes = _outcomes.putIfAbsent(page, () => <String?, KinetoOutcome>{});
    outcomes.remove(target);
    outcomes[target] = outcome;
    _last = outcome;
    _finished++;
    notifyListeners();
    return problem == null;
  }

  /// How the last request asked for in [page] about [target] ended; with no
  /// [target], the newest of the page's that failed.
  KinetoOutcome? _outcomeOn(String page, String? target) {
    final outcomes = _outcomes[page];
    if (outcomes == null) return null;
    if (target != null) return outcomes[target];
    for (final outcome in outcomes.values.toList().reversed) {
      if (outcome.problem != null) return outcome;
    }
    return null;
  }

  /// Why the last request asked for in [page] about [target] failed, if it
  /// did; with no [target], why the newest failed request there did.
  String? problemOn(String page, {String? target}) => _outcomeOn(page, target)?.problem;

  /// Asks again for the request whose failure [problemOn] gives, when that
  /// is offered.
  VoidCallback? retryOn(String page, {String? target}) {
    final last = _outcomeOn(page, target);
    if (last == null || last.retry == null) return null;
    return () => run(page, last.task.label, last.retry!, target: last.task.target, retry: last.retry);
  }

  /// Forget how the requests asked for in [page] ended, only the one about
  /// [target] when given; with no [page], how the most recent one did.
  void dismiss([String? page, String? target]) {
    var changed = false;
    if (page != null) {
      changed = target == null ? _outcomes.remove(page)?.isNotEmpty ?? false : _outcomes[page]?.remove(target) != null;
    }
    final last = _last;
    if (last != null && (page == null || (last.task.page == page && (target == null || last.task.target == target)))) {
      _last = null;
      changed = true;
    }
    if (changed) notifyListeners();
  }
}

/// Says what the controls below work on, such as `shot:2`, so a button
/// shows only the work asked for about that.
class KinetoTargetScope extends InheritedWidget {
  const KinetoTargetScope({required this.target, required super.child, super.key});

  final String target;

  static String? of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<KinetoTargetScope>()?.target;

  @override
  bool updateShouldNotify(KinetoTargetScope old) => old.target != target;
}

/// Makes the project's [KinetoWork] reachable from any button, dialogs
/// included.
class KinetoWorkScope extends InheritedNotifier<KinetoWork> {
  const KinetoWorkScope({required KinetoWork? work, required super.child, super.key}) : super(notifier: work);

  static KinetoWork? maybeOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<KinetoWorkScope>()?.notifier;
}

/// The strip along the bottom of the window: the requests under way and how
/// long each has run, or how the last one ended, whatever workspace is open.
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

  /// How many requests the bar names; the rest are counted.
  static const int _shown = 3;

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
    final running = widget.work?.busy ?? false;
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
    final running = work?.running ?? const <KinetoTask>[];
    final last = work?.last;
    final style = TextStyle(color: WabTheme.mutedColor, fontSize: 13, fontFamilyFallback: kWabKaiFallback);
    String where(String page) => widget.pageLabel?.call(page) ?? page;

    final children = <Widget>[];
    if (running.isNotEmpty) {
      children.addAll(<Widget>[
        SizedBox.square(dimension: 12, child: CircularProgressIndicator(strokeWidth: 1.6, color: WabTheme.accentColor)),
        const SizedBox(width: 10),
      ]);
      for (final (index, task) in running.take(_shown).indexed) {
        final waiting = work!.isWaiting(task);
        children.add(
          Flexible(
            child: InkWell(
              key: ValueKey<String>('status-task-$index'),
              onTap: widget.onShow == null ? null : () => widget.onShow!(task.page),
              child: Padding(
                padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 4),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: <Widget>[
                    Flexible(
                      child: KinetoWorkingText(
                        waiting ? 'Waiting · ${task.label}' : task.label,
                        style: style.copyWith(color: waiting ? WabTheme.mutedColor : WabTheme.textColor),
                        overflow: TextOverflow.ellipsis,
                      ),
                    ),
                    Flexible(
                      child: Text(
                        '  ${_elapsed(DateTime.now().difference(task.started))} · ${where(task.page)}',
                        style: style,
                        overflow: TextOverflow.ellipsis,
                        softWrap: false,
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        );
        if (index < running.length - 1) children.add(Text('  |  ', style: style));
      }
      if (running.length > _shown) children.add(Text('and ${running.length - _shown} more', style: style));
      children.add(const Spacer());
      if (widget.onShow != null) {
        children.add(
          TextButton(
            key: const ValueKey<String>('status-show'),
            onPressed: () => widget.onShow!(running.first.page),
            child: Text('Open ${where(running.first.page)}', style: style.copyWith(color: WabTheme.accentColor)),
          ),
        );
      }
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
