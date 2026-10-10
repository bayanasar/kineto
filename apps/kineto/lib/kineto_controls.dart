import 'package:flutter/material.dart';
import 'package:wabisabi/wabisabi.dart';

import 'kineto_work.dart';

/// Kineto's button: a small plain block of colour. Gold for the action a
/// screen is for and for the option chosen, grey for everything else. It
/// casts a soft shadow by day and stands in lamplight by night.
///
/// The request a button starts shows on it, with the project's work rather
/// than the page's, so it is still there after the director looks at
/// another workspace and comes back.
///
/// A prototype while the director settles the look, drawn only in
/// WabiSabi's colours: the plain button moves into WabiSabi beside its
/// material ones (docs/playbooks/wabisabi-ui-consumer.md), and this keeps
/// only the part that knows Kineto's work.
class KinetoButton extends StatefulWidget {
  const KinetoButton({
    required this.child,
    this.onPressed,
    this.primary = false,
    this.expand = false,
    this.padding = defaultPadding,
    super.key,
  });

  final Widget child;
  final VoidCallback? onPressed;

  /// The action the screen is for, or the option that is chosen.
  final bool primary;

  /// Takes the whole width it is given.
  final bool expand;

  final EdgeInsets padding;

  static const EdgeInsets defaultPadding = EdgeInsets.symmetric(horizontal: 14, vertical: 6);

  @override
  State<KinetoButton> createState() => _KinetoButtonState();
}

class _KinetoButtonState extends State<KinetoButton> {
  bool _hovered = false;
  bool _pressed = false;

  @override
  Widget build(BuildContext context) {
    // WabiSabi keeps its palette in statics the theme sets; depending on
    // the theme redraws this when the director switches it.
    Theme.of(context);
    final dark = WabTheme.isDark;
    final work = KinetoWorkScope.maybeOf(context);
    final task = work?.startedBy(widget.key, target: KinetoTargetScope.of(context));
    final busy = task != null;
    final enabled = widget.onPressed != null && !busy;

    final base = widget.primary ? WabTheme.accentColor : WabTheme.secondaryColor;
    final ink = widget.primary ? (dark ? WabTheme.backgroundColor : WabTheme.textColor) : WabTheme.textColor;
    final fill = switch ((enabled, _pressed, _hovered)) {
      (true, true, _) => Color.lerp(base, ink, .14)!,
      (true, false, true) => Color.lerp(base, WabTheme.paperWhite, .12)!,
      _ => base,
    };
    // Lifted off the page: a soft shadow below it by day, lamplight behind
    // it by night. Hovering lifts it further; pressing sets it down.
    final lift = !(enabled || busy)
        ? 0.0
        : _pressed
            ? .5
            : _hovered
                ? 1.6
                : 1.0;
    final glow = lift == 0
        ? null
        : dark
            ? <BoxShadow>[
                BoxShadow(
                  color: WabTheme.accentColor.withValues(alpha: (.30 * lift).clamp(0, .55)),
                  blurRadius: 8 + 5 * lift,
                  spreadRadius: .5 * lift,
                ),
              ]
            : <BoxShadow>[
                BoxShadow(
                  color: WabTheme.textColor.withValues(alpha: (.20 * lift).clamp(0, .34)),
                  blurRadius: 3 + 3 * lift,
                  offset: Offset(0, 1 + 1.4 * lift),
                ),
              ];
    final style = (Theme.of(context).textTheme.labelLarge ?? const TextStyle()).copyWith(
      color: ink,
      fontSize: 13.5,
      fontWeight: FontWeight.w600,
      letterSpacing: .4,
      fontFamilyFallback: kWabKaiFallback,
      decoration: TextDecoration.none,
    );

    // A request that waits for another about the same thing, or for a
    // place among those under way, says so until it starts.
    Widget label = busy ? KinetoWorkingText(work!.isWaiting(task) ? 'Waiting' : task.short) : widget.child;
    label = DefaultTextStyle(
      style: style,
      textAlign: TextAlign.center,
      child: IconTheme.merge(
        data: IconThemeData(color: ink, size: 16),
        child: label,
      ),
    );
    if (widget.expand) label = Center(child: label);

    Widget button = AnimatedContainer(
      duration: const Duration(milliseconds: 90),
      width: widget.expand ? double.infinity : null,
      padding: widget.padding,
      decoration: BoxDecoration(color: fill, borderRadius: BorderRadius.circular(4), boxShadow: glow),
      child: label,
    );
    if (!enabled && !busy) button = Opacity(opacity: .42, child: button);

    return Semantics(
      button: true,
      enabled: enabled,
      child: MouseRegion(
        cursor: enabled ? SystemMouseCursors.click : SystemMouseCursors.basic,
        onEnter: (_) => setState(() => _hovered = true),
        onExit: (_) => setState(() {
          _hovered = false;
          _pressed = false;
        }),
        child: GestureDetector(
          onTapDown: enabled ? (_) => setState(() => _pressed = true) : null,
          onTapCancel: enabled ? () => setState(() => _pressed = false) : null,
          onTap: enabled
              ? () {
                  setState(() => _pressed = false);
                  work?.arm(widget.key);
                  try {
                    widget.onPressed!();
                  } finally {
                    work?.arm(null);
                  }
                }
              : null,
          child: button,
        ),
      ),
    );
  }
}

/// The sun and moon in the corner: the sun takes a dark screen to light, the
/// moon a light one to dark.
class KinetoThemeToggle extends StatelessWidget {
  const KinetoThemeToggle({required this.isDarkMode, required this.onToggle, super.key});

  final bool isDarkMode;
  final VoidCallback? onToggle;

  @override
  Widget build(BuildContext context) {
    final label = isDarkMode ? 'Light mode' : 'Dark mode';
    // An app bar stretches its actions to its height; the key keeps its size.
    return Center(
      child: Padding(
        padding: const EdgeInsets.only(right: 16),
        child: Tooltip(
          message: label,
          child: Semantics(
            label: label,
            child: KinetoButton(
              key: const ValueKey<String>('theme-toggle'),
              onPressed: onToggle,
              padding: const EdgeInsets.all(7),
              child: Icon(isDarkMode ? Icons.light_mode_outlined : Icons.dark_mode_outlined),
            ),
          ),
        ),
      ),
    );
  }
}
