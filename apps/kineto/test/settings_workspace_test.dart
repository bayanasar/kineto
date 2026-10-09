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
  WidgetController.hitTestWarningShouldBeFatal = true;
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
        loadSettings: () async => const KinetoProviderSettings(),
        loadKeys: (_) async => const KinetoKeys(unavailable: null, states: <String, KinetoKeyState>{}),
        loadPlugins: () {
          reads++;
          return reads == 1 ? plugins.future : Future<List<KinetoPlugin>>.value(const <KinetoPlugin>[]);
        },
        loadServers: () => Future<List<KinetoMcpServer>>.error(
          const KinetoProjectException(165, 'The plugin configuration (plugins.toml) could not be read'),
        ),
      ),
    );
    expect(
      find.text('Reading'),
      findsNWidgets(4),
      reason: 'providers, keys, plugins and servers are read off the screen',
    );
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

  testWidgets('a key is saved into the secure storage, cleared from the field, and removed', (tester) async {
    final stored = <String, String>{};
    final deleted = <String>[];
    final asked = <List<String>>[];
    KinetoKeys keys(List<String> names) {
      asked.add(names);
      return KinetoKeys(
        unavailable: null,
        states: <String, KinetoKeyState>{
          for (final name in names)
            name: KinetoKeyState(stored: stored.containsKey(name), inEnvironment: name == 'gemini'),
        },
      );
    }

    await _pump(
      tester,
      SettingsWorkspaceScreen(
        loadSettings: () async => const KinetoProviderSettings(),
        loadKeys: (names) async => keys(names),
        storeKey: (name, key) async => stored[name] = key,
        deleteKey: (name) async {
          deleted.add(name);
          stored.remove(name);
        },
        loadPlugins: () async => const <KinetoPlugin>[],
        loadServers: () async => const <KinetoMcpServer>[],
      ),
    );
    await tester.pumpAndSettle();
    expect(asked.first, <String>['minimax', 'gemini', 'kimi', 'veo']);
    expect(find.text('Secure storage'), findsOneWidget);
    expect(find.text('Read from KINETO_GEMINI_API_KEY, since the secure storage holds none.'), findsOneWidget);
    expect(find.byKey(const ValueKey<String>('key-delete-minimax')), findsNothing);

    final input = find.byKey(const ValueKey<String>('key-input-minimax'));
    expect(tester.widget<TextField>(input).obscureText, isTrue);
    await tester.enterText(input, '  sk-live-0123  ');
    await tester.tap(find.byKey(const ValueKey<String>('key-save-minimax')));
    await tester.pumpAndSettle();
    expect(stored, <String, String>{'minimax': 'sk-live-0123'});
    expect(tester.widget<TextField>(input).controller!.text, isEmpty, reason: 'the key is not kept on screen');
    expect(find.textContaining('sk-live'), findsNothing);
    expect(find.text('MiniMax key saved. Projects opened from now on use it.'), findsOneWidget);
    expect(find.text('Replace'), findsOneWidget);

    await tester.tap(find.byKey(const ValueKey<String>('key-delete-minimax')));
    await tester.pumpAndSettle();
    expect(deleted, <String>['minimax']);
    expect(find.byKey(const ValueKey<String>('key-delete-minimax')), findsNothing);
  });

  testWidgets('without secure storage, keys come from the environment and none is asked for', (tester) async {
    await _pump(
      tester,
      SettingsWorkspaceScreen(
        loadSettings: () async => const KinetoProviderSettings(),
        loadKeys: (_) async => const KinetoKeys(
          unavailable: 'no secure storage is available: the platform failed (no Secret Service)',
          states: <String, KinetoKeyState>{
            'minimax': KinetoKeyState(stored: false, inEnvironment: true),
          },
        ),
        loadPlugins: () async => const <KinetoPlugin>[],
        loadServers: () async => const <KinetoMcpServer>[],
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('No secure storage'), findsOneWidget);
    expect(find.byKey(const ValueKey<String>('keys-unavailable')), findsOneWidget);
    expect(
      find.descendant(of: find.byKey(const ValueKey<String>('keys')), matching: find.byType(TextField)),
      findsNothing,
    );
    expect(find.text('From the environment'), findsOneWidget);
  });

  testWidgets('providers are chosen here: the agents on this computer, a server, and what the environment overrides',
      (tester) async {
    var settings = const KinetoProviderSettings(
      values: <KinetoProviderSetting, String>{KinetoProviderSetting.languageModel: 'claude-code'},
      overridden: <KinetoProviderSetting>{KinetoProviderSetting.image},
      agents: <KinetoAgent, String>{KinetoAgent.claudeCode: '/usr/local/bin/claude', KinetoAgent.agy: '/opt/agy'},
    );
    final chosen = <Map<KinetoProviderSetting, String>>[];
    final asked = <List<String>>[];
    await _pump(
      tester,
      SettingsWorkspaceScreen(
        loadSettings: () async => settings,
        setSetting: (setting, value) async {
          chosen.add(<KinetoProviderSetting, String>{setting: value});
          settings = KinetoProviderSettings(
            values: <KinetoProviderSetting, String>{...settings.values, setting: value},
            agents: settings.agents,
          );
        },
        loadKeys: (names) async {
          asked.add(names);
          return const KinetoKeys(unavailable: null, states: <String, KinetoKeyState>{});
        },
        loadPlugins: () async => _plugins,
        loadServers: () async => _servers.toList(),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('Claude Code · installed'), findsOneWidget);
    expect(find.text('Runs /usr/local/bin/claude'), findsOneWidget);
    expect(find.text('KINETO_IMAGE_PROVIDER is set for this run and wins over this choice.'), findsOneWidget);
    expect(asked.last, contains('DEMO_KEY'), reason: "a plugin's secrets are kept by their own names");

    await tester.tap(find.byKey(const ValueKey<String>('choose-language_model')));
    await tester.pumpAndSettle();
    expect(
      tester.widget<DropdownMenuItem<String>>(
        find.byKey(const ValueKey<String>('choice-language_model-codex')).last,
      ).enabled,
      isFalse,
      reason: 'an agent that is not installed is not offered',
    );
    final server = find.text('OpenAI-compatible server').last;
    await tester.ensureVisible(server);
    await tester.pumpAndSettle();
    await tester.tap(server);
    await tester.pumpAndSettle();
    expect(chosen.last, <KinetoProviderSetting, String>{KinetoProviderSetting.languageModel: 'openai-compatible'});

    await tester.enterText(find.byKey(const ValueKey<String>('server-url')), 'https://api.deepseek.example/v1');
    await tester.enterText(find.byKey(const ValueKey<String>('server-model')), 'deepseek-chat');
    await tester.enterText(find.byKey(const ValueKey<String>('server-key')), 'deepseek');
    await tester.tap(find.byKey(const ValueKey<String>('server-save')));
    await tester.pumpAndSettle();
    expect(settings.value(KinetoProviderSetting.endpointUrl), 'https://api.deepseek.example/v1');
    expect(settings.value(KinetoProviderSetting.endpointKey), 'deepseek');
    expect(asked.last, contains('deepseek'), reason: "the server's key is listed under the name it was given");
    expect(find.text('deepseek · The OpenAI-compatible server chosen for documents'), findsOneWidget);
    expect(find.text('Saved. Projects opened from now on use it.'), findsOneWidget);

    await tester.tap(find.byKey(const ValueKey<String>('choose-image')));
    await tester.pumpAndSettle();
    final agy = find.text('Antigravity (agy) · installed').last;
    await tester.ensureVisible(agy);
    await tester.pumpAndSettle();
    await tester.tap(agy);
    await tester.pumpAndSettle();
    expect(chosen.last, <KinetoProviderSetting, String>{KinetoProviderSetting.image: 'agy'});
    expect(find.text('agy runs your own hooks with each request.'), findsOneWidget);
  });
}
