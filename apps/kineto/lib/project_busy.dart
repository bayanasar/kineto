import 'package:flutter/material.dart';
import 'package:wabisabi/wabisabi.dart';

import 'kineto_controls.dart';

/// Shown instead of the workspace when another Kineto window already holds the
/// project's single-writer lock.
class KinetoProjectBusyApp extends StatelessWidget {
  const KinetoProjectBusyApp({required this.projectPath, super.key});

  final String projectPath;

  @override
  Widget build(BuildContext context) {
    return _ProjectRefusalApp(
      title: 'Project already open',
      message:
          'The project at $projectPath is open in another Kineto window. '
          'Close that window, then start Kineto again. Two windows writing '
          "the same project would overwrite each other's approvals.",
    );
  }
}

/// Shown instead of the workspace when the project's filesystem cannot provide
/// the single-writer lock, so Kineto refuses to write it.
class KinetoProjectLockUnavailableApp extends StatelessWidget {
  const KinetoProjectLockUnavailableApp({required this.projectPath, super.key});

  final String projectPath;

  @override
  Widget build(BuildContext context) {
    return _ProjectRefusalApp(
      title: 'Project cannot be locked',
      message:
          'The filesystem holding $projectPath cannot lock files, so Kineto '
          'cannot guarantee it is the only window writing this project and '
          'will not open it for writing. Move the project to a local disk.',
    );
  }
}

class _ProjectRefusalApp extends StatelessWidget {
  const _ProjectRefusalApp({required this.title, required this.message});

  final String title;
  final String message;

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Kineto',
      theme: WabTheme.materialTheme(lightTheme: false),
      home: WabScaffold(
        appBar: WabAppBar(title: const Text('Kineto')),
        body: Center(
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 640),
            child: Padding(
              padding: const EdgeInsets.all(24),
              child: WabPanel(title: title, child: Text(message)),
            ),
          ),
        ),
      ),
    );
  }
}

/// Shown in place of a workspace while the project opens again with new
/// provider choices, or when it could not be opened with them. Settings stay
/// a click away, so a choice that cannot be used here is changed where it
/// was made.
class ProjectUnavailableScreen extends StatelessWidget {
  const ProjectUnavailableScreen({
    required this.opening,
    this.problem,
    this.navigation,
    this.isDarkMode = true,
    this.onThemeToggle,
    super.key,
  });

  final bool opening;
  final String? problem;
  final Widget? navigation;
  final bool isDarkMode;
  final VoidCallback? onThemeToggle;

  @override
  Widget build(BuildContext context) => WabScaffold(
        appBar: WabAppBar(
          title: const Text('Kineto'),
          action: onThemeToggle == null ? null : KinetoThemeToggle(isDarkMode: isDarkMode, onToggle: onThemeToggle),
        ),
        body: SingleChildScrollView(
          padding: const EdgeInsets.all(24),
          child: Center(
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 1040),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: <Widget>[
                  if (navigation != null) ...<Widget>[navigation!, const SizedBox(height: 20)],
                  if (opening)
                    const Align(
                      alignment: Alignment.centerLeft,
                      child: WabStatusBadge('Opening the project with your choices', kind: WabBadgeKind.progress),
                    )
                  else
                    WabPanel(
                      title: 'The project could not be opened',
                      child: Text(
                        '${problem ?? 'Kineto could not open the project.'} '
                        'Change the providers under Settings; the project opens again when you come back.',
                      ),
                    ),
                ],
              ),
            ),
          ),
        ),
      );
}
