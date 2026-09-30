import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/project_busy.dart';

void main() {
  testWidgets('busy project explains the other window instead of opening', (
    tester,
  ) async {
    await tester.pumpWidget(
      const KinetoProjectBusyApp(projectPath: '/state/kineto/demo-project'),
    );

    expect(find.text('Project already open'), findsOneWidget);
    expect(
      find.textContaining('/state/kineto/demo-project'),
      findsOneWidget,
    );
  });

  testWidgets('unlockable filesystem refuses to open the project for writing', (
    tester,
  ) async {
    await tester.pumpWidget(
      const KinetoProjectLockUnavailableApp(
        projectPath: '/mnt/share/demo-project',
      ),
    );

    expect(find.text('Project cannot be locked'), findsOneWidget);
    expect(find.textContaining('/mnt/share/demo-project'), findsOneWidget);
  });
}
