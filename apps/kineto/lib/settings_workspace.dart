import 'package:flutter/material.dart';
import 'package:wabisabi/wabisabi.dart';

import 'native/kineto_engine.dart';
import 'screenplay_workspace.dart';

/// The provider plugins and MCP servers the user registered, read only:
/// both are registered in Kineto's configuration folder, never through the
/// app, and this page never asks for a program to run.
class SettingsWorkspaceScreen extends StatefulWidget {
  const SettingsWorkspaceScreen({
    this.loadPlugins = loadKinetoPluginsInBackground,
    this.loadServers = loadKinetoMcpServersInBackground,
    this.navigation,
    this.isDarkMode = true,
    this.onThemeToggle,
    super.key,
  });

  /// Reads the plugins; starts each enabled one, so it runs off the UI
  /// thread.
  final Future<List<KinetoPlugin>> Function() loadPlugins;

  /// Reads the MCP servers and their tools, likewise.
  final Future<List<KinetoMcpServer>> Function() loadServers;
  final Widget? navigation;
  final bool isDarkMode;
  final VoidCallback? onThemeToggle;

  @override
  State<SettingsWorkspaceScreen> createState() => _SettingsWorkspaceScreenState();
}

class _SettingsWorkspaceScreenState extends State<SettingsWorkspaceScreen> {
  List<KinetoPlugin>? _plugins;
  List<KinetoMcpServer>? _servers;
  String? _pluginsProblem;
  String? _serversProblem;

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    setState(() {
      _plugins = null;
      _servers = null;
      _pluginsProblem = null;
      _serversProblem = null;
    });
    String problem(Object error) => error is KinetoProjectException ? error.message : error.toString();
    Future<void> plugins() async {
      try {
        _plugins = await widget.loadPlugins();
      } on Object catch (error) {
        _pluginsProblem = problem(error);
      }
    }

    Future<void> servers() async {
      try {
        _servers = await widget.loadServers();
      } on Object catch (error) {
        _serversProblem = problem(error);
      }
    }

    await Future.wait(<Future<void>>[plugins(), servers()]);
    if (mounted) setState(() {});
  }

  @override
  Widget build(BuildContext context) => SettingsWorkspaceView(
        plugins: _plugins,
        pluginsProblem: _pluginsProblem,
        servers: _servers,
        serversProblem: _serversProblem,
        onReload: _load,
        navigation: widget.navigation,
        isDarkMode: widget.isDarkMode,
        onThemeToggle: widget.onThemeToggle,
      );
}

class SettingsWorkspaceView extends StatelessWidget {
  const SettingsWorkspaceView({
    this.plugins,
    this.pluginsProblem,
    this.servers,
    this.serversProblem,
    this.onReload,
    this.navigation,
    this.isDarkMode = true,
    this.onThemeToggle,
    super.key,
  });

  /// Null while they are read.
  final List<KinetoPlugin>? plugins;
  final String? pluginsProblem;
  final List<KinetoMcpServer>? servers;
  final String? serversProblem;
  final VoidCallback? onReload;
  final Widget? navigation;
  final bool isDarkMode;
  final VoidCallback? onThemeToggle;

  @override
  Widget build(BuildContext context) => WabScaffold(
        appBar: WabAppBar(
          title: const Text('Kineto'),
          action: onThemeToggle == null
              ? null
              : WabToggleButton(
                  text: const Text('Dark mode'),
                  isOn: isDarkMode,
                  pair: WabTogglePair.sealZhuwen,
                  callback: onThemeToggle,
                ),
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
                  const Text(
                    'Plugins and MCP servers are programs you install yourself and register in Kineto\'s '
                    'configuration folder (KINETO_CONFIG_HOME, or kineto under your configuration '
                    'directory): plugins.toml and mcp.toml. Kineto never asks for a program here, and '
                    'a project can never name one. Each is started only to answer one request, with '
                    'only the secrets its entry names.',
                  ),
                  const SizedBox(height: 12),
                  Align(
                    alignment: Alignment.centerLeft,
                    child: WabButton(
                      key: const ValueKey<String>('settings-reload'),
                      kind: WabMaterialKind.paper,
                      onPressed: plugins == null && pluginsProblem == null ? null : onReload,
                      child: const Text('Read the configuration again'),
                    ),
                  ),
                  const SizedBox(height: 20),
                  _pluginsPanel(context),
                  const SizedBox(height: 20),
                  _serversPanel(context),
                ],
              ),
            ),
          ),
        ),
      );

  Widget _pluginsPanel(BuildContext context) {
    final plugins = this.plugins;
    return WabPanel(
      title: 'Provider plugins',
      trailing: plugins == null
          ? WabStatusBadge(pluginsProblem == null ? 'Reading' : 'Unreadable',
              kind: pluginsProblem == null ? WabBadgeKind.progress : WabBadgeKind.warning)
          : WabStatusBadge('${plugins.length} registered', kind: WabBadgeKind.neutral),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          const Text(
            'A plugin makes images, video or speech as a built-in provider does, under the same '
            'rules: nothing is paid for twice unasked. Use one with KINETO_IMAGE_PROVIDER=plugin:<id>; '
            'video and speech follow it when it makes them.',
          ),
          if (pluginsProblem != null) ...<Widget>[const SizedBox(height: 12), Text(pluginsProblem!)],
          if (plugins != null && plugins.isEmpty) ...<Widget>[
            const SizedBox(height: 12),
            const Text('None registered.'),
          ],
          for (final plugin in plugins ?? const <KinetoPlugin>[]) ...<Widget>[
            const SizedBox(height: 12),
            _plugin(context, plugin),
          ],
        ],
      ),
    );
  }

  static String _price(int? micros, String unit) =>
      micros == null ? 'price unknown' : micros == 0 ? 'free' : '${formatDollars(micros)} $unit';

  Widget _plugin(BuildContext context, KinetoPlugin plugin) {
    final description = plugin.description;
    final (status, kind) = !plugin.enabled
        ? ('Disabled', WabBadgeKind.neutral)
        : description == null
            ? ('Not answering', WabBadgeKind.warning)
            : plugin.selected
                ? ('In use', WabBadgeKind.done)
                : ('Ready', WabBadgeKind.neutral);
    return WabPanel(
      key: ValueKey<String>('plugin-${plugin.id}'),
      title: description == null ? plugin.id : '${plugin.id} · ${description.name} ${description.version}',
      trailing: WabStatusBadge(status, kind: kind),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          if (description != null) ...<Widget>[
            if (description.makesImages)
              Text(
                'Images · ${description.imageModel} · ${description.aspectRatios.join(', ')}'
                '${description.identityReferences == 0 ? '' : ' · up to ${description.identityReferences} identity references'}'
                '${description.compositionReference ? ' · composition reference' : ''}'
                ' · ${_price(description.imagePriceMicros, 'an image')}',
              ),
            if (description.makesVideos)
              Text(
                'Video · ${description.videoModel} · '
                '${description.clipSeconds.map((seconds) => '$seconds s').join(', ')} clips',
              ),
            if (description.speaks)
              Text(
                'Speech · ${description.speechModel} · ${description.languages.join(', ')}'
                ' · ${_price(description.speechCharacterPriceMicros, 'a character')}',
              ),
          ],
          if (plugin.problem.isNotEmpty) Text(plugin.problem),
          Text('Runs ${plugin.executable}'),
          if (plugin.secrets.isNotEmpty) Text('Given ${plugin.secrets.join(', ')}'),
          Text('Selected as ${plugin.selection}'),
        ],
      ),
    );
  }

  Widget _serversPanel(BuildContext context) {
    final servers = this.servers;
    return WabPanel(
      title: 'MCP servers',
      trailing: servers == null
          ? WabStatusBadge(serversProblem == null ? 'Reading' : 'Unreadable',
              kind: serversProblem == null ? WabBadgeKind.progress : WabBadgeKind.warning)
          : WabStatusBadge('${servers.length} registered', kind: WabBadgeKind.neutral),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          const Text(
            'A server\'s tools bring outside material into the bible, as references you can read, '
            'edit and attach. Only the tools its entry allows are offered, and only when you ask, '
            'from the bible. It cannot ask Kineto for anything.',
          ),
          if (serversProblem != null) ...<Widget>[const SizedBox(height: 12), Text(serversProblem!)],
          if (servers != null && servers.isEmpty) ...<Widget>[
            const SizedBox(height: 12),
            const Text('None registered.'),
          ],
          for (final server in servers ?? const <KinetoMcpServer>[]) ...<Widget>[
            const SizedBox(height: 12),
            _server(context, server),
          ],
        ],
      ),
    );
  }

  Widget _server(BuildContext context, KinetoMcpServer server) {
    final tools = server.tools;
    final (status, kind) = !server.enabled
        ? ('Disabled', WabBadgeKind.neutral)
        : tools == null
            ? ('Not answering', WabBadgeKind.warning)
            : ('${tools.length} of ${server.allowed.length} tools', WabBadgeKind.neutral);
    return WabPanel(
      key: ValueKey<String>('mcp-${server.id}'),
      title: server.id,
      trailing: WabStatusBadge(status, kind: kind),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          if (server.problem.isNotEmpty) Text(server.problem),
          Text('Runs ${server.executable}'),
          if (server.secrets.isNotEmpty) Text('Given ${server.secrets.join(', ')}'),
          Text('Allows ${server.allowed.join(', ')}'),
          for (final tool in tools ?? const <KinetoMcpTool>[]) ...<Widget>[
            const SizedBox(height: 8),
            Text(tool.name, style: Theme.of(context).textTheme.titleSmall),
            if (tool.description.isNotEmpty) Text(tool.description),
            for (final parameter in tool.parameters)
              Text(
                '· ${parameter.name}${parameter.required ? ' (required)' : ''}'
                '${parameter.kind == null ? ' · cannot be given here' : ''}'
                '${parameter.description.isEmpty ? '' : ': ${parameter.description}'}',
              ),
          ],
        ],
      ),
    );
  }
}
