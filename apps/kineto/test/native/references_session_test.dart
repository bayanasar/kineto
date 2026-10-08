import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/native/kineto_engine.dart';

Matcher _refusedWith(int code) =>
    throwsA(isA<KinetoProjectException>().having((error) => error.code, 'code', code));

void main() {
  test('references cross the native session and refuse what is not registered or kept', () async {
    final sandbox = Directory.systemTemp.createTempSync('kineto-references-ffi-');
    try {
      final session = await KinetoProjectSession.createTextProject(
        path: '${sandbox.path}${Platform.pathSeparator}film',
        projectId: 'references_ffi',
        title: 'References',
        createdAt: '2026-10-04T00:00:00Z',
        language: 'en',
        sourceText: 'A filmmaker waits.\n',
      );
      expect(session.references, isEmpty);
      // No server of that name is registered: refused before anything runs.
      await expectLater(
        session.askReference(
          server: 'kineto-test-no-such-server',
          tool: 'search_wardrobe',
          arguments: const <KinetoToolArgument>[
            KinetoToolArgument('query', 'the filmmaker'),
            KinetoToolArgument('limit', '3', kind: KinetoToolArgumentKind.number),
          ],
          attachedTo: const <String>[kinetoReferenceWorld],
        ),
        _refusedWith(KinetoProjectException.mcpNotAllowed),
      );
      await expectLater(
        session.editReference('reference.0001', 'A grey coat.'),
        _refusedWith(KinetoProjectException.noSuchArtifact),
      );
      await expectLater(
        session.attachReference('reference.0001', kinetoReferenceWorld),
        _refusedWith(KinetoProjectException.noSuchArtifact),
      );
      await expectLater(
        session.removeReference('reference.0001'),
        _refusedWith(KinetoProjectException.noSuchArtifact),
      );
      expect(session.references, isEmpty);
      session.close();
    } finally {
      sandbox.deleteSync(recursive: true);
    }
  });

  test('the registered plugins and MCP servers are read off the UI isolate', () async {
    // What is registered depends on the machine; reading it must not fail.
    expect(await loadKinetoPluginsInBackground(), isA<List<KinetoPlugin>>());
    expect(await loadKinetoMcpServersInBackground(), isA<List<KinetoMcpServer>>());
  });
}
