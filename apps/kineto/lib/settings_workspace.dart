import 'package:flutter/material.dart';
import 'package:wabisabi/wabisabi.dart';

import 'native/kineto_engine.dart';
import 'screenplay_workspace.dart';

/// Provider keys, kept in the system's secure storage, and the provider
/// plugins and MCP servers the user registered, read only: both are
/// registered in Kineto's configuration folder, never through the app, and
/// this page never asks for a program to run.
class SettingsWorkspaceScreen extends StatefulWidget {
  const SettingsWorkspaceScreen({
    this.loadKeys = loadKinetoKeysInBackground,
    this.storeKey = storeKinetoKeyInBackground,
    this.deleteKey = deleteKinetoKeyInBackground,
    this.loadPlugins = loadKinetoPluginsInBackground,
    this.loadServers = loadKinetoMcpServersInBackground,
    this.navigation,
    this.isDarkMode = true,
    this.onThemeToggle,
    super.key,
  });

  /// Reads where each provider's key is; opening the secure storage can
  /// wait on the user, so these run off the UI thread.
  final Future<KinetoKeys> Function() loadKeys;
  final Future<void> Function(KinetoKeyProvider provider, String key) storeKey;
  final Future<void> Function(KinetoKeyProvider provider) deleteKey;

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
  KinetoKeys? _keys;
  String? _keysProblem;
  KinetoKeyProvider? _keyWorking;
  String? _keyNote;
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

    await Future.wait(<Future<void>>[_readKeys(), plugins(), servers()]);
    if (mounted) setState(() {});
  }

  static String _problem(Object error) => error is KinetoProjectException ? error.message : error.toString();

  Future<void> _readKeys() async {
    try {
      _keys = await widget.loadKeys();
      _keysProblem = null;
    } on Object catch (error) {
      _keysProblem = _problem(error);
    }
  }

  /// Store or remove a key, then read the keys again. A key goes to the
  /// engine once and is not kept here.
  Future<void> _changeKey(KinetoKeyProvider provider, Future<void> Function() change, String done) async {
    setState(() {
      _keyWorking = provider;
      _keyNote = null;
    });
    String note;
    try {
      await change();
      note = done;
    } on Object catch (error) {
      note = _problem(error);
    }
    await _readKeys();
    if (!mounted) return;
    setState(() {
      _keyWorking = null;
      _keyNote = note;
    });
  }

  @override
  Widget build(BuildContext context) => SettingsWorkspaceView(
        keys: _keys,
        keysProblem: _keysProblem,
        keyWorking: _keyWorking,
        keyNote: _keyNote,
        onStoreKey: (provider, key) => _changeKey(
          provider,
          () => widget.storeKey(provider, key),
          '${provider.label} key saved. Projects opened from now on use it.',
        ),
        onDeleteKey: (provider) => _changeKey(
          provider,
          () => widget.deleteKey(provider),
          '${provider.label} key removed. Projects opened from now on do without it.',
        ),
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
    this.keys,
    this.keysProblem,
    this.keyWorking,
    this.keyNote,
    this.onStoreKey,
    this.onDeleteKey,
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
  final KinetoKeys? keys;
  final String? keysProblem;

  /// The provider whose key is being stored or removed.
  final KinetoKeyProvider? keyWorking;

  /// What the last change of a key came to.
  final String? keyNote;
  final void Function(KinetoKeyProvider provider, String key)? onStoreKey;
  final void Function(KinetoKeyProvider provider)? onDeleteKey;

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
                  _keysPanel(context),
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

  Widget _keysPanel(BuildContext context) {
    final keys = this.keys;
    return WabPanel(
      key: const ValueKey<String>('keys'),
      title: 'Provider keys',
      trailing: keys == null
          ? WabStatusBadge(keysProblem == null ? 'Reading' : 'Unreadable',
              kind: keysProblem == null ? WabBadgeKind.progress : WabBadgeKind.warning)
          : keys.available
              ? const WabStatusBadge('Secure storage', kind: WabBadgeKind.done)
              : const WabStatusBadge('No secure storage', kind: WabBadgeKind.warning),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          const Text(
            'A key is kept in your system\'s secure storage (the macOS Keychain, the Windows '
            'Credential Manager, or a Secret Service such as GNOME Keyring on Linux), never in a '
            'project or a file, and is never shown again. One set in the environment is read when '
            'the storage holds none. Projects opened after a change use it.',
          ),
          if (keysProblem != null) ...<Widget>[const SizedBox(height: 12), Text(keysProblem!)],
          if (keys != null && !keys.available) ...<Widget>[
            const SizedBox(height: 12),
            Text(
              '${keys.unavailable}. Keys cannot be saved here; set them in the environment before '
              'opening Kineto.',
              key: const ValueKey<String>('keys-unavailable'),
            ),
          ],
          if (keyNote != null) ...<Widget>[
            const SizedBox(height: 12),
            Text(keyNote!, key: const ValueKey<String>('keys-note')),
          ],
          if (keys != null)
            for (final provider in KinetoKeyProvider.values) ...<Widget>[
              const SizedBox(height: 12),
              _KeyRow(
                key: ValueKey<String>('key-${provider.wireName}'),
                provider: provider,
                state: keys.states[provider],
                problem: keys.problems[provider],
                canStore: keys.available,
                working: keyWorking == provider,
                idle: keyWorking == null,
                onStore: onStoreKey == null ? null : (key) => onStoreKey!(provider, key),
                onDelete: onDeleteKey == null ? null : () => onDeleteKey!(provider),
              ),
            ],
        ],
      ),
    );
  }

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

/// One provider's key: where it is, and a field to save a new one, which is
/// cleared once it is handed on.
class _KeyRow extends StatefulWidget {
  const _KeyRow({
    required this.provider,
    required this.state,
    required this.problem,
    required this.canStore,
    required this.working,
    required this.idle,
    required this.onStore,
    required this.onDelete,
    super.key,
  });

  final KinetoKeyProvider provider;
  final KinetoKeyState? state;
  final String? problem;
  final bool canStore;
  final bool working;
  final bool idle;
  final ValueChanged<String>? onStore;
  final VoidCallback? onDelete;

  @override
  State<_KeyRow> createState() => _KeyRowState();
}

class _KeyRowState extends State<_KeyRow> {
  final TextEditingController _key = TextEditingController();

  @override
  void dispose() {
    _key.dispose();
    super.dispose();
  }

  void _save() {
    final key = _key.text.trim();
    _key.clear();
    if (key.isNotEmpty) widget.onStore?.call(key);
  }

  @override
  Widget build(BuildContext context) {
    final provider = widget.provider;
    final state = widget.state;
    final (status, kind) = widget.working
        ? ('Working', WabBadgeKind.progress)
        : widget.problem != null
            ? ('Unreadable', WabBadgeKind.warning)
            : state?.stored ?? false
                ? ('Saved', WabBadgeKind.done)
                : state?.inEnvironment ?? false
                    ? ('From the environment', WabBadgeKind.neutral)
                    : ('Not set', WabBadgeKind.neutral);
    final stored = state?.stored ?? false;
    final ready = widget.canStore && widget.idle && widget.onStore != null;
    return WabPanel(
      title: '${provider.label} · ${provider.usedFor}',
      trailing: WabStatusBadge(status, kind: kind),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          if (widget.problem != null) Text(widget.problem!),
          if (!stored && (state?.inEnvironment ?? false))
            Text('Read from ${provider.variable}, since the secure storage holds none.'),
          if (widget.canStore) ...<Widget>[
            const SizedBox(height: 8),
            Row(
              children: <Widget>[
                Expanded(
                  child: TextField(
                    key: ValueKey<String>('key-input-${provider.wireName}'),
                    controller: _key,
                    obscureText: true,
                    enableSuggestions: false,
                    autocorrect: false,
                    enabled: ready,
                    decoration: InputDecoration(
                      labelText: stored ? 'New ${provider.label} key' : '${provider.label} key',
                    ),
                    onSubmitted: (_) => _save(),
                  ),
                ),
                const SizedBox(width: 12),
                WabButton(
                  key: ValueKey<String>('key-save-${provider.wireName}'),
                  kind: WabMaterialKind.seal,
                  onPressed: ready ? _save : null,
                  child: Text(stored ? 'Replace' : 'Save'),
                ),
                if (stored) ...<Widget>[
                  const SizedBox(width: 12),
                  WabButton(
                    key: ValueKey<String>('key-delete-${provider.wireName}'),
                    kind: WabMaterialKind.zhuwen,
                    onPressed: widget.idle && widget.onDelete != null ? widget.onDelete : null,
                    child: const Text('Remove'),
                  ),
                ],
              ],
            ),
          ],
        ],
      ),
    );
  }
}
