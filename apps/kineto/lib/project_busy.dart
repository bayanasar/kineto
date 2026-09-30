import 'package:flutter/material.dart';
import 'package:wabisabi/wabisabi.dart';

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
