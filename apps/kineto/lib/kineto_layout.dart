import 'package:flutter/material.dart';
import 'package:wabisabi/wabisabi.dart';

import 'kineto_controls.dart';
import 'kineto_work.dart';

/// How wide the window must be for a workspace's two columns side by side.
const double kTwoColumnWidth = 1000;

/// A workspace laid out for a desktop window: what the director starts from
/// or picks among in a narrower column on the left, the work itself on the
/// right, each scrolling on its own. Notices, such as a refusal, run across
/// both. A narrow window stacks the columns.
class KinetoWorkspace extends StatelessWidget {
  const KinetoWorkspace({
    required this.title,
    required this.side,
    required this.main,
    this.notices = const <Widget>[],
    this.sideWidth = 420,
    this.sideKey,
    this.mainKey,
    super.key,
  });

  /// The workspace's name, at the top.
  final String title;
  final List<Widget> notices;
  final List<Widget> side;
  final List<Widget> main;
  final double sideWidth;

  /// Keys of the two scrolling columns.
  final Key? sideKey;
  final Key? mainKey;

  static const EdgeInsets _gutter = EdgeInsets.fromLTRB(24, 0, 24, 24);

  static List<Widget> _spaced(List<Widget> panels) => <Widget>[
        for (final (index, panel) in panels.indexed) ...<Widget>[
          if (index > 0) const SizedBox(height: 20),
          panel,
        ],
      ];

  Widget _column(List<Widget> panels, Key? key, EdgeInsets padding) => SingleChildScrollView(
        key: key,
        padding: padding,
        child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: _spaced(panels)),
      );

  @override
  Widget build(BuildContext context) {
    // WabiSabi keeps its palette in statics the theme sets.
    Theme.of(context);
    final header = Padding(
      padding: const EdgeInsets.fromLTRB(24, 20, 24, 16),
      child: Text(
        title,
        key: const ValueKey<String>('workspace-title'),
        style: TextStyle(
          color: WabTheme.textColor,
          fontSize: 20,
          fontWeight: FontWeight.w700,
          letterSpacing: .3,
          fontFamilyFallback: kWabKaiFallback,
        ),
      ),
    );
    return Scaffold(
      body: LayoutBuilder(
        builder: (context, constraints) {
          final wide = constraints.maxWidth >= kTwoColumnWidth && side.isNotEmpty && main.isNotEmpty;
          if (!wide) {
            return _column(
              <Widget>[...notices, ...side, ...main],
              mainKey,
              _gutter.copyWith(top: 0),
            ).withHeader(header);
          }
          return Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: <Widget>[
              header,
              if (notices.isNotEmpty)
                Padding(
                  padding: const EdgeInsets.fromLTRB(24, 0, 24, 20),
                  child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: _spaced(notices)),
                ),
              Expanded(
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: <Widget>[
                    SizedBox(
                      width: sideWidth,
                      child: _column(side, sideKey, const EdgeInsets.fromLTRB(24, 0, 12, 24)),
                    ),
                    Expanded(child: _column(main, mainKey, const EdgeInsets.fromLTRB(12, 0, 24, 24))),
                  ],
                ),
              ),
            ],
          );
        },
      ),
    );
  }
}

extension on Widget {
  /// This scrolling column under [header], which stays put.
  Widget withHeader(Widget header) => Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[header, Expanded(child: this)],
      );
}

/// A place in the sidebar: a workspace, by the name it is shown under.
@immutable
class KinetoPlace {
  const KinetoPlace({required this.page, required this.label, required this.icon});

  /// The page name requests are asked for under, such as `production`.
  final String page;
  final String label;
  final IconData icon;
}

/// The left side of the window: the workspaces, one above another, the one
/// open marked, each saying when work it asked for is under way; the
/// project's name above them and the theme key below.
class KinetoSidebar extends StatelessWidget {
  const KinetoSidebar({
    required this.places,
    required this.current,
    required this.onShow,
    this.projectTitle,
    this.isDarkMode = true,
    this.onThemeToggle,
    super.key,
  });

  final List<KinetoPlace> places;

  /// The page open now.
  final String current;
  final ValueChanged<String> onShow;
  final String? projectTitle;
  final bool isDarkMode;
  final VoidCallback? onThemeToggle;

  static const double width = 232;

  @override
  Widget build(BuildContext context) {
    Theme.of(context);
    final work = KinetoWorkScope.maybeOf(context);
    return Material(
      key: const ValueKey<String>('sidebar'),
      color: WabTheme.surfaceColor,
      child: Container(
        width: width,
        decoration: BoxDecoration(border: Border(right: BorderSide(color: WabTheme.lineColor, width: .6))),
        child: WabSidebar(
          width: width,
          padding: const EdgeInsets.fromLTRB(16, 20, 16, 16),
          children: <Widget>[
            Text(
              'Kineto',
              style: TextStyle(
                color: WabTheme.textColor,
                fontSize: 22,
                fontWeight: FontWeight.w700,
                letterSpacing: .6,
                fontFamilyFallback: kWabKaiFallback,
              ),
            ),
            if (projectTitle != null) ...<Widget>[
              const SizedBox(height: 4),
              Text(
                projectTitle!,
                key: const ValueKey<String>('project-title'),
                style: TextStyle(color: WabTheme.mutedColor, fontSize: 13, fontFamilyFallback: kWabKaiFallback),
                overflow: TextOverflow.ellipsis,
              ),
            ],
            const SizedBox(height: 24),
            for (final place in places) ...<Widget>[
              KinetoButton(
                key: ValueKey<String>('workspace-${place.page}'),
                primary: place.page == current,
                expand: true,
                padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 9),
                onPressed: () => onShow(place.page),
                child: Row(
                  children: <Widget>[
                    Icon(place.icon, size: 17),
                    const SizedBox(width: 10),
                    Expanded(child: Text(place.label, textAlign: TextAlign.start, overflow: TextOverflow.ellipsis)),
                    if (work?.busyOn(place.page) ?? false)
                      KinetoWorkingText('', key: ValueKey<String>('working-${place.page}')),
                  ],
                ),
              ),
              const SizedBox(height: 6),
            ],
            const Spacer(),
            if (onThemeToggle != null)
              Align(
                alignment: Alignment.centerLeft,
                child: KinetoThemeToggle(isDarkMode: isDarkMode, onToggle: onThemeToggle),
              ),
          ],
        ),
      ),
    );
  }
}
