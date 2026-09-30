import 'package:flutter/material.dart';
import 'package:wabisabi/wabisabi.dart';

/// Shown instead of the workspace when another Kineto window already holds the
/// project's single-writer lock.
class KinetoProjectBusyApp extends StatelessWidget {
  const KinetoProjectBusyApp({required this.projectPath, super.key});

  final String projectPath;

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
              child: WabPanel(
                title: 'Project already open',
                child: Text(
                  'The project at $projectPath is open in another Kineto window. '
                  'Close that window, then start Kineto again. Two windows writing '
                  "the same project would overwrite each other's approvals.",
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}
