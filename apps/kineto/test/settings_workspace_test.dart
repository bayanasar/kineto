import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/native/kineto_engine.dart';
import 'package:kineto/settings_workspace.dart';
import 'package:wabisabi/wabisabi.dart';

const List<KinetoPlugin> _plugins = <KinetoPlugin>[
  KinetoPlugin(
    id: 'demo',
    executable: '/opt/kineto/kineto-demo-plugin',
    secrets: <String>['DEMO_KEY'],
    selected: true,
    description: KinetoPluginDescription(
      name: 'Kineto demo plugin',
      version: '0.1.0',
      imageModel: 'demo-image',
      aspectRatios: <String>['1:1', '16:9'],
      identityReferences: 2,
      compositionReference: true,
      imagePriceMicros: 0,
      speechModel: 'demo-speech',
      languages: <String>['en', 'zh'],
      videoModel: 'demo-video',
      clipSeconds: <int>[6],
    ),
  ),
  KinetoPlugin(id: 'old', executable: '/opt/old', enabled: false),
  KinetoPlugin(id: 'broken', executable: '/opt/broken', problem: 'the answer is not JSON'),
];

const List<KinetoMcpServer> _servers = <KinetoMcpServer>[
  KinetoMcpServer(
    id: 'studio-archive',
    executable: '/opt/studio/archive-mcp',
    allowed: <String>['search_wardrobe', 'location_notes'],
    tools: <KinetoMcpTool>[
      KinetoMcpTool(
        name: 'search_wardrobe',
        description: "Search the studio's wardrobe notes.",
        parameters: <KinetoMcpParameter>[
          KinetoMcpParameter(name: 'query', kind: KinetoToolArgumentKind.text, required: true),
          KinetoMcpParameter(name: 'filters'),
        ],
      ),
    ],
  ),
];

Future<void> _pump(WidgetTester tester, Widget view) async {
  tester.view.physicalSize = const Size(1280, 2400);
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  await tester.pumpWidget(MaterialApp(theme: WabTheme.materialTheme(lightTheme: false), home: view));
}

void main() {
  testWidgets('plugins and servers are shown as registered, never edited', (tester) async {
    await _pump(tester, const SettingsWorkspaceView(plugins: _plugins, servers: _servers));
    expect(find.text('3 registered'), findsOneWidget);
    expect(find.text('demo · Kineto demo plugin 0.1.0'), findsOneWidget);
    expect(find.text('In use'), findsOneWidget);
    expect(
      find.text('Images · demo-image · 1:1, 16:9 · up to 2 identity references · composition reference · free'),
      findsOneWidget,
    );
    expect(find.text('Video · demo-video · 6 s clips'), findsOneWidget);
    expect(find.text('Speech · demo-speech · en, zh · price unknown'), findsOneWidget);
    expect(find.text('Given DEMO_KEY'), findsOneWidget);
    expect(find.text('Selected as plugin:demo'), findsOneWidget);
    expect(find.text('Disabled'), findsOneWidget);
    expect(find.text('Not answering'), findsOneWidget);
    expect(find.text('the answer is not JSON'), findsOneWidget);

    expect(find.text('1 registered'), findsOneWidget);
    expect(find.text('1 of 2 tools'), findsOneWidget);
    expect(find.text('Allows search_wardrobe, location_notes'), findsOneWidget);
    expect(find.text('· query (required)'), findsOneWidget);
    expect(find.text('· filters · cannot be given here'), findsOneWidget);
    // Nothing here takes a program to run.
    expect(find.byType(TextField), findsNothing);
  });

  testWidgets('the configuration is read off the screen and again on request', (tester) async {
    var reads = 0;
    final plugins = Completer<List<KinetoPlugin>>();
    await _pump(
      tester,
      SettingsWorkspaceScreen(
        loadPlugins: () {
          reads++;
          return reads == 1 ? plugins.future : Future<List<KinetoPlugin>>.value(const <KinetoPlugin>[]);
        },
        loadServers: () => Future<List<KinetoMcpServer>>.error(
          const KinetoProjectException(165, 'The plugin configuration (plugins.toml) could not be read'),
        ),
      ),
    );
    expect(find.text('Reading'), findsNWidgets(2));
    plugins.complete(_plugins);
    await tester.pumpAndSettle();
    expect(find.text('3 registered'), findsOneWidget);
    expect(find.text('Unreadable'), findsOneWidget);
    expect(find.text('The plugin configuration (plugins.toml) could not be read'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey<String>('settings-reload')));
    await tester.pumpAndSettle();
    expect(reads, 2);
    expect(find.text('None registered.'), findsOneWidget);
  });
}
