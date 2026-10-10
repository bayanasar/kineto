import 'package:flutter/material.dart';
import 'package:wabisabi/wabisabi.dart';

import 'kineto_controls.dart';
import 'kineto_layout.dart';
import 'native/kineto_engine.dart';
import 'screenplay_workspace.dart';

/// Which providers make each part of a film, the services the director
/// added by the protocol they speak, the keys they take, kept in the
/// system's secure storage, and the provider plugins and MCP servers the
/// user registered, read only: both are registered in Kineto's configuration
/// folder, never through the app, and this page never asks for a program to
/// run.
class SettingsWorkspaceScreen extends StatefulWidget {
  const SettingsWorkspaceScreen({
    this.loadSettings = loadKinetoProviderSettingsInBackground,
    this.setSetting = setKinetoProviderSettingInBackground,
    this.putConnection = putKinetoConnectionInBackground,
    this.removeConnection = removeKinetoConnectionInBackground,
    this.loadKeys = loadKinetoKeysInBackground,
    this.storeKey = storeKinetoKeyInBackground,
    this.deleteKey = deleteKinetoKeyInBackground,
    this.loadPlugins = loadKinetoPluginsInBackground,
    this.loadServers = loadKinetoMcpServersInBackground,
    this.onChanged,
    super.key,
  });

  /// Reads the provider choices and looks for agent tools; changes one.
  /// Both touch files, so they run off the UI thread.
  final Future<KinetoProviderSettings> Function() loadSettings;
  final Future<void> Function(KinetoProviderSetting setting, String value) setSetting;

  /// Adds a connection or puts it in place of the one named `previous`;
  /// removes one. Both write the settings file.
  final Future<void> Function(KinetoConnection connection, {String previous}) putConnection;
  final Future<void> Function(String name) removeConnection;

  /// Reads where each named key is; opening the secure storage can wait on
  /// the user, so these run off the UI thread.
  final Future<KinetoKeys> Function(List<String> names) loadKeys;
  final Future<void> Function(String name, String key) storeKey;
  final Future<void> Function(String name) deleteKey;

  /// Reads the plugins; starts each enabled one, so it runs off the UI
  /// thread.
  final Future<List<KinetoPlugin>> Function() loadPlugins;

  /// Reads the MCP servers and their tools, likewise.
  final Future<List<KinetoMcpServer>> Function() loadServers;

  /// Told when a choice, a key or the configuration changed, so the open
  /// project takes it up when the director goes back to it.
  final VoidCallback? onChanged;

  @override
  State<SettingsWorkspaceScreen> createState() => _SettingsWorkspaceScreenState();
}

class _SettingsWorkspaceScreenState extends State<SettingsWorkspaceScreen> {
  KinetoProviderSettings? _settings;
  String? _settingsProblem;
  bool _settingWorking = false;
  String? _settingNote;
  KinetoKeys? _keys;
  String? _keysProblem;
  String? _keyWorking;
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
      _keys = null;
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

    await Future.wait(<Future<void>>[_readSettings(), plugins(), servers()]);
    if (mounted) setState(() {});
    await _readKeys();
    if (mounted) setState(() {});
  }

  static String _problem(Object error) => error is KinetoProjectException ? error.message : error.toString();

  Future<void> _readSettings() async {
    try {
      _settings = await widget.loadSettings();
      _settingsProblem = null;
    } on Object catch (error) {
      _settingsProblem = _problem(error);
    }
  }

  /// The keys settings list: the built-in providers', each one a
  /// connection takes, and every secret a plugin or MCP server entry lists.
  List<KinetoKeySlot> get _slots {
    final slots = <KinetoKeySlot>[...KinetoKeySlot.builtIn];
    void add(KinetoKeySlot slot) {
      if (!slots.contains(slot)) slots.add(slot);
    }

    final users = <String, List<String>>{};
    for (final connection in _settings?.connections ?? const <KinetoConnection>[]) {
      if (connection.key.isNotEmpty) (users[connection.key] ??= <String>[]).add(connection.name);
    }
    for (final MapEntry(key: name, value: connections) in users.entries) {
      add(KinetoKeySlot.connection(name, connections));
    }
    for (final plugin in _plugins ?? const <KinetoPlugin>[]) {
      for (final secret in plugin.secrets) {
        add(KinetoKeySlot.secret(secret, 'the ${plugin.id} plugin'));
      }
    }
    for (final server in _servers ?? const <KinetoMcpServer>[]) {
      for (final secret in server.secrets) {
        add(KinetoKeySlot.secret(secret, 'the ${server.id} MCP server'));
      }
    }
    return slots;
  }

  Future<void> _readKeys() async {
    try {
      _keys = await widget.loadKeys(<String>[for (final slot in _slots) slot.name]);
      _keysProblem = null;
    } on Object catch (error) {
      _keysProblem = _problem(error);
    }
  }

  /// Make one or more choices, then read the settings and keys again.
  Future<void> _choose(Map<KinetoProviderSetting, String> choices) async {
    setState(() {
      _settingWorking = true;
      _settingNote = null;
    });
    String note = 'Saved. Your project uses it as soon as you go back to it.';
    try {
      for (final MapEntry(key: setting, value: value) in choices.entries) {
        await widget.setSetting(setting, value);
        widget.onChanged?.call();
      }
    } on Object catch (error) {
      note = _problem(error);
    }
    await _readSettings();
    await _readKeys();
    if (!mounted) return;
    setState(() {
      _settingWorking = false;
      _settingNote = note;
    });
  }

  /// Add or change a connection, keeping [key] under its key's name when
  /// one is given; or remove one. Then read the settings and keys again.
  Future<void> _changeConnection(Future<void> Function() change, String done) async {
    setState(() {
      _settingWorking = true;
      _settingNote = null;
    });
    String note = done;
    try {
      await change();
      widget.onChanged?.call();
    } on Object catch (error) {
      note = _problem(error);
    }
    await _readSettings();
    await _readKeys();
    if (!mounted) return;
    setState(() {
      _settingWorking = false;
      _settingNote = note;
    });
  }

  Future<void> _putConnection(KinetoConnection connection, String previous, String key) => _changeConnection(
        () async {
          await widget.putConnection(connection, previous: previous);
          if (key.isNotEmpty && connection.key.isNotEmpty) await widget.storeKey(connection.key, key);
        },
        'The ${connection.name} connection is saved. Your project uses it as soon as you go back to it.',
      );

  Future<void> _removeConnection(KinetoConnection connection) => _changeConnection(
        () => widget.removeConnection(connection.name),
        'The ${connection.name} connection is removed. Its key stays under Keys until you remove it.',
      );

  /// Store or remove a key, then read the keys again. A key goes to the
  /// engine once and is not kept here.
  Future<void> _changeKey(String name, Future<void> Function() change, String done) async {
    setState(() {
      _keyWorking = name;
      _keyNote = null;
    });
    String note;
    try {
      await change();
      widget.onChanged?.call();
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
        settings: _settings,
        settingsProblem: _settingsProblem,
        settingWorking: _settingWorking,
        settingNote: _settingNote,
        onChoose: _choose,
        onPutConnection: _putConnection,
        onRemoveConnection: _removeConnection,
        slots: _slots,
        keys: _keys,
        keysProblem: _keysProblem,
        keyWorking: _keyWorking,
        keyNote: _keyNote,
        onStoreKey: (slot, key) => _changeKey(
          slot.name,
          () => widget.storeKey(slot.name, key),
          '${slot.label} key saved. Your project uses it as soon as you go back to it.',
        ),
        onDeleteKey: (slot) => _changeKey(
          slot.name,
          () => widget.deleteKey(slot.name),
          '${slot.label} key removed. Your project does without it as soon as you go back to it.',
        ),
        plugins: _plugins,
        pluginsProblem: _pluginsProblem,
        servers: _servers,
        serversProblem: _serversProblem,
        onReload: () {
          // plugins.toml and mcp.toml name providers too.
          widget.onChanged?.call();
          _load();
        },
      );
}

/// What a connection can be added for.
const List<KinetoModality> _connectable = KinetoModality.values;

class SettingsWorkspaceView extends StatelessWidget {
  const SettingsWorkspaceView({
    this.settings,
    this.settingsProblem,
    this.settingWorking = false,
    this.settingNote,
    this.onChoose,
    this.onPutConnection,
    this.onRemoveConnection,
    this.slots = KinetoKeySlot.builtIn,
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
    super.key,
  });

  /// The provider choices; null while they are read.
  final KinetoProviderSettings? settings;
  final String? settingsProblem;

  /// Whether a choice is being saved.
  final bool settingWorking;

  /// What the last choice came to.
  final String? settingNote;
  final void Function(Map<KinetoProviderSetting, String> choices)? onChoose;

  /// Saves a connection, in place of the one named by the second argument
  /// when it is not empty, with the key typed for it, if any.
  final void Function(KinetoConnection connection, String previous, String key)? onPutConnection;
  final void Function(KinetoConnection connection)? onRemoveConnection;

  /// The keys to list, in order.
  final List<KinetoKeySlot> slots;

  /// Null while they are read.
  final KinetoKeys? keys;
  final String? keysProblem;

  /// The name of the key being stored or removed.
  final String? keyWorking;

  /// What the last change of a key came to.
  final String? keyNote;
  final void Function(KinetoKeySlot slot, String key)? onStoreKey;
  final void Function(KinetoKeySlot slot)? onDeleteKey;

  /// Null while they are read.
  final List<KinetoPlugin>? plugins;
  final String? pluginsProblem;
  final List<KinetoMcpServer>? servers;
  final String? serversProblem;
  final VoidCallback? onReload;

  @override
  Widget build(BuildContext context) => KinetoWorkspace(
        title: 'Settings',
        sideWidth: 560,
        // Who makes what: the providers, and the keys they need.
        side: <Widget>[
          _ProvidersPanel(
            key: const ValueKey<String>('providers'),
            settings: settings,
            problem: settingsProblem,
            working: settingWorking,
            note: settingNote,
            plugins: plugins ?? const <KinetoPlugin>[],
            onChoose: onChoose,
          ),
          for (final modality in _connectable)
            _ConnectionsPanel(
              key: ValueKey<String>('connections-${modality.name}'),
              modality: modality,
              settings: settings,
              working: settingWorking,
              keys: keys,
              onPut: onPutConnection,
              onRemove: onRemoveConnection,
            ),
          _keysPanel(context),
        ],
        // The programs the director registered.
        main: <Widget>[
          const Text(
            'Plugins and MCP servers are programs you install yourself and register in Kineto\'s '
            'configuration folder (KINETO_CONFIG_HOME, or kineto under your configuration '
            'directory): plugins.toml and mcp.toml. Kineto never asks for a program here, and '
            'a project can never name one. Each is started only to answer one request, with '
            'only the secrets its entry names.',
          ),
          Align(
            alignment: Alignment.centerLeft,
            child: KinetoButton(
              key: const ValueKey<String>('settings-reload'),
              onPressed: plugins == null && pluginsProblem == null ? null : onReload,
              child: const Text('Read the configuration again'),
            ),
          ),
          _pluginsPanel(context),
          _serversPanel(context),
        ],
      );

  Widget _keysPanel(BuildContext context) {
    final keys = this.keys;
    return WabPanel(
      key: const ValueKey<String>('keys'),
      title: 'Keys',
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
            for (final slot in slots) ...<Widget>[
              const SizedBox(height: 12),
              _KeyRow(
                key: ValueKey<String>('key-${slot.name}'),
                slot: slot,
                state: keys.states[slot.name],
                problem: keys.problems[slot.name],
                canStore: keys.available,
                working: keyWorking == slot.name,
                idle: keyWorking == null,
                onStore: onStoreKey == null ? null : (key) => onStoreKey!(slot, key),
                onDelete: onDeleteKey == null ? null : () => onDeleteKey!(slot),
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
            'rules: nothing is paid for twice unasked. Choose one under Providers above.',
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
                '${description.styleReference ? ' · style reference' : ''}'
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
    required this.slot,
    required this.state,
    required this.problem,
    required this.canStore,
    required this.working,
    required this.idle,
    required this.onStore,
    required this.onDelete,
    super.key,
  });

  final KinetoKeySlot slot;
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
    final slot = widget.slot;
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
      title: '${slot.label} · ${slot.usedFor}',
      trailing: WabStatusBadge(status, kind: kind),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          if (widget.problem != null) Text(widget.problem!),
          if (!stored && (state?.inEnvironment ?? false))
            Text('Read from ${slot.variable}, since the secure storage holds none.'),
          if (widget.canStore) ...<Widget>[
            const SizedBox(height: 8),
            Row(
              children: <Widget>[
                Expanded(
                  child: TextField(
                    key: ValueKey<String>('key-input-${slot.name}'),
                    controller: _key,
                    obscureText: true,
                    enableSuggestions: false,
                    autocorrect: false,
                    enabled: ready,
                    decoration: InputDecoration(
                      labelText: stored ? 'New ${slot.label} key' : '${slot.label} key',
                    ),
                    onSubmitted: (_) => _save(),
                  ),
                ),
                const SizedBox(width: 12),
                KinetoButton(
                  key: ValueKey<String>('key-save-${slot.name}'),
                  primary: true,
                  onPressed: ready ? _save : null,
                  child: Text(stored ? 'Replace' : 'Save'),
                ),
                if (stored) ...<Widget>[
                  const SizedBox(width: 12),
                  KinetoButton(
                    key: ValueKey<String>('key-delete-${slot.name}'),
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

/// One thing a provider setting can be, as the director reads it.
typedef _Option = ({String value, String label, bool enabled});

/// Which provider makes each part of a film.
class _ProvidersPanel extends StatelessWidget {
  const _ProvidersPanel({
    required this.settings,
    required this.problem,
    required this.working,
    required this.note,
    required this.plugins,
    required this.onChoose,
    super.key,
  });

  final KinetoProviderSettings? settings;
  final String? problem;
  final bool working;
  final String? note;
  final List<KinetoPlugin> plugins;
  final void Function(Map<KinetoProviderSetting, String> choices)? onChoose;

  String _agentLabel(KinetoAgent agent) =>
      '${agent.label} · ${settings?.agents.containsKey(agent) ?? false ? 'installed' : 'not found'}';

  _Option _connectionOption(KinetoConnection connection) =>
      (value: connection.selection, label: '${connection.name} · ${connection.model}', enabled: true);

  List<_Option> _options(KinetoProviderSetting setting) {
    final agents = settings?.agents ?? const <KinetoAgent, String>{};
    List<KinetoConnection> connections(KinetoModality modality) =>
        settings?.connectionsFor(modality) ?? const <KinetoConnection>[];
    final current = settings?.value(setting) ?? '';
    final described = <KinetoPlugin>[
      for (final plugin in plugins)
        if (plugin.enabled && plugin.description != null) plugin,
    ];
    final options = <_Option>[
      ...switch (setting) {
        KinetoProviderSetting.languageModel => <_Option>[
            (value: '', label: 'None: documents cannot be written', enabled: true),
            for (final agent in KinetoAgent.values)
              (
                value: agent.selection,
                label: _agentLabel(agent),
                enabled: agents.containsKey(agent) || current == agent.selection,
              ),
            (value: 'kimi', label: 'Kimi API · key', enabled: true),
            for (final connection in connections(KinetoModality.text)) _connectionOption(connection),
          ],
        KinetoProviderSetting.image => <_Option>[
            (value: '', label: 'Local demo · free', enabled: true),
            (value: 'minimax', label: 'MiniMax image-01 · key', enabled: true),
            for (final connection in connections(KinetoModality.image)) _connectionOption(connection),
            for (final agent in KinetoAgent.values)
              if (agent.makesImages)
                (
                  value: agent.selection,
                  label: _agentLabel(agent),
                  enabled: agents.containsKey(agent) || current == agent.selection,
                ),
            for (final plugin in described)
              if (plugin.description!.makesImages)
                (value: plugin.selection, label: '${plugin.id} plugin', enabled: true),
          ],
        KinetoProviderSetting.video => <_Option>[
            (value: '', label: 'Follows images', enabled: true),
            (value: 'local-demo', label: 'Local demo · free', enabled: true),
            (value: 'minimax', label: 'MiniMax Hailuo · key', enabled: true),
            (value: 'veo', label: 'Veo 3.1 Lite · key', enabled: true),
            (value: 'veo:fast', label: 'Veo 3.1 Fast · key', enabled: true),
            (value: 'veo:standard', label: 'Veo 3.1 · key', enabled: true),
            (value: 'omni', label: 'Gemini Omni Flash · key', enabled: true),
            for (final plugin in described)
              if (plugin.description!.makesVideos)
                (value: plugin.selection, label: '${plugin.id} plugin', enabled: true),
          ],
        KinetoProviderSetting.speech => <_Option>[
            (value: '', label: 'Follows images', enabled: true),
            (value: 'local-demo', label: 'Local demo · free', enabled: true),
            (value: 'minimax', label: 'MiniMax speech · key', enabled: true),
            for (final plugin in described)
              if (plugin.description!.speaks)
                (value: plugin.selection, label: '${plugin.id} plugin', enabled: true),
          ],
      },
    ];
    if (!options.any((option) => option.value == current)) {
      options.add((value: current, label: '$current (as set)', enabled: true));
    }
    return options;
  }

  Widget _choice(BuildContext context, KinetoProviderSetting setting, String title) {
    final settings = this.settings!;
    final ready = onChoose != null && !working && settings.problem == null;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: <Widget>[
        Row(
          children: <Widget>[
            SizedBox(width: 160, child: Text(title)),
            Expanded(
              child: DropdownButton<String>(
                key: ValueKey<String>('choose-${setting.wireName}'),
                value: settings.value(setting),
                isExpanded: true,
                items: <DropdownMenuItem<String>>[
                  for (final option in _options(setting))
                    DropdownMenuItem<String>(
                      key: ValueKey<String>('choice-${setting.wireName}-${option.value}'),
                      value: option.value,
                      enabled: option.enabled,
                      child: Text(option.label),
                    ),
                ],
                onChanged: ready
                    ? (value) {
                        if (value != null && value != settings.value(setting)) {
                          onChoose!(<KinetoProviderSetting, String>{setting: value});
                        }
                      }
                    : null,
              ),
            ),
          ],
        ),
        if (settings.overridden.contains(setting))
          Text('${setting.variable} is set for this run and wins over this choice.'),
      ],
    );
  }

  @override
  Widget build(BuildContext context) {
    final settings = this.settings;
    final model = settings?.value(KinetoProviderSetting.languageModel) ?? '';
    final agent = KinetoAgent.values.where((agent) => agent.selection == model).firstOrNull;
    return WabPanel(
      title: 'Providers',
      trailing: settings == null
          ? WabStatusBadge(problem == null ? 'Reading' : 'Unreadable',
              kind: problem == null ? WabBadgeKind.progress : WabBadgeKind.warning)
          : working
              ? const WabStatusBadge('Saving', kind: WabBadgeKind.progress)
              : settings.problem != null
                  ? const WabStatusBadge('Unreadable', kind: WabBadgeKind.warning)
                  : const WabStatusBadge('Chosen here', kind: WabBadgeKind.neutral),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          const Text(
            'Choose who makes each part. The agent tools you already use are found on this computer '
            'and run under your own sign-in: Kineto starts them without tools and never reads their '
            'sign-in. Any other service is added below as a connection, by the protocol it speaks. '
            'Your project uses these choices as soon as you go back to it.',
          ),
          if (problem != null) ...<Widget>[const SizedBox(height: 12), Text(problem!)],
          if (settings?.problem != null) ...<Widget>[
            const SizedBox(height: 12),
            Text(
              '${settings!.problem}. Nothing is changed here until the file can be read.',
              key: const ValueKey<String>('providers-unreadable'),
            ),
          ],
          if (note != null) ...<Widget>[
            const SizedBox(height: 12),
            Text(note!, key: const ValueKey<String>('providers-note')),
          ],
          if (settings != null) ...<Widget>[
            const SizedBox(height: 12),
            _choice(context, KinetoProviderSetting.languageModel, 'Documents'),
            if (agent != null && settings.agents[agent] != null)
              Text('Runs ${settings.agents[agent]}', key: const ValueKey<String>('agent-path')),
            if (agent?.caveat != null) Text(agent!.caveat!),
            const SizedBox(height: 12),
            _choice(context, KinetoProviderSetting.image, 'Images'),
            if (settings.value(KinetoProviderSetting.image) == KinetoAgent.agy.selection &&
                KinetoAgent.agy.caveat != null)
              Text(KinetoAgent.agy.caveat!),
            const SizedBox(height: 12),
            _choice(context, KinetoProviderSetting.video, 'Video'),
            const SizedBox(height: 12),
            _choice(context, KinetoProviderSetting.speech, 'Speech'),
          ],
        ],
      ),
    );
  }
}

/// What a connection costs, in words.
String _priceLabel(KinetoConnection connection) {
  final modality = connection.protocol?.modality;
  if (modality == KinetoModality.image) {
    return connection.pricePerImage.isEmpty
        ? 'No price given: costs show as unknown'
        : '\$${connection.pricePerImage} an image';
  }
  if (connection.pricePerMillionInput.isEmpty || connection.pricePerMillionOutput.isEmpty) {
    return 'No price given: costs show as unknown';
  }
  return '\$${connection.pricePerMillionInput} in, \$${connection.pricePerMillionOutput} out, '
      'per million tokens';
}

/// Why [connection] cannot be saved as typed, in words; null when it can be
/// sent for the engine to check.
String? _connectionProblem(KinetoConnection connection) {
  final name = RegExp(r'^[a-z_][a-z0-9_]{0,63}$');
  if (!name.hasMatch(connection.name)) {
    return 'A name is lowercase letters, digits and _, such as deepseek.';
  }
  final url = connection.url;
  final local = RegExp(r'^http://(localhost|127\.\d+\.\d+\.\d+|\[::1\])(:\d+)?(/|$)');
  if (!(url.startsWith('https://') && url.length > 'https://'.length) && !local.hasMatch(url)) {
    return 'An address starts with https://, or http:// to this computer.';
  }
  if (connection.model.isEmpty || connection.model.contains(RegExp(r'\s'))) {
    return 'Name the model in one word, such as deepseek-chat.';
  }
  if (connection.key.isNotEmpty && !name.hasMatch(connection.key)) {
    return 'A key name is lowercase letters, digits and _, such as deepseek.';
  }
  for (final price in <String>[
    connection.pricePerMillionInput,
    connection.pricePerMillionOutput,
    connection.pricePerImage,
  ]) {
    final dollars = double.tryParse(price);
    if (price.isNotEmpty && (dollars == null || dollars < 0 || dollars > 10000)) {
      return 'A price is a number of US dollars, such as 0.28.';
    }
  }
  if (connection.references < 0 || connection.references > 16) {
    return 'A connection takes at most 16 reference images.';
  }
  return null;
}

/// The connections that make one thing: each as it is, with where its key
/// is, and one form to add a connection or change one.
class _ConnectionsPanel extends StatefulWidget {
  const _ConnectionsPanel({
    required this.modality,
    required this.settings,
    required this.working,
    required this.keys,
    required this.onPut,
    required this.onRemove,
    super.key,
  });

  final KinetoModality modality;

  /// Null while they are read.
  final KinetoProviderSettings? settings;
  final bool working;

  /// Where each key is; null while they are read.
  final KinetoKeys? keys;
  final void Function(KinetoConnection connection, String previous, String key)? onPut;
  final void Function(KinetoConnection connection)? onRemove;

  @override
  State<_ConnectionsPanel> createState() => _ConnectionsPanelState();
}

class _ConnectionsPanelState extends State<_ConnectionsPanel> {
  /// The name of the connection being changed, empty while one is added;
  /// null when the form is closed.
  String? _editing;
  late KinetoProtocol _protocol = _protocols.first;

  /// Whether the key's name still follows the connection's.
  bool _keyFollows = false;

  /// Whether the form was sent and its outcome is not yet read back.
  bool _sent = false;
  String? _problem;
  final TextEditingController _name = TextEditingController();
  final TextEditingController _url = TextEditingController();
  final TextEditingController _model = TextEditingController();
  final TextEditingController _keyName = TextEditingController();
  final TextEditingController _key = TextEditingController();
  final TextEditingController _input = TextEditingController();
  final TextEditingController _output = TextEditingController();
  final TextEditingController _image = TextEditingController();
  final TextEditingController _references = TextEditingController();

  List<KinetoProtocol> get _protocols =>
      KinetoProtocol.values.where((protocol) => protocol.modality == widget.modality).toList();

  @override
  void initState() {
    super.initState();
    _name.addListener(() {
      if (_keyFollows && _keyName.text != _name.text) _keyName.text = _name.text;
    });
  }

  @override
  void didUpdateWidget(_ConnectionsPanel old) {
    super.didUpdateWidget(old);
    // Once the settings are read back, the form closes if the connection
    // is there as it was sent, and stays open to be corrected if not.
    if (_sent && !widget.working) {
      _sent = false;
      final saved = widget.settings?.connections.where((connection) => connection.name == _name.text.trim());
      if (saved != null && saved.isNotEmpty && saved.first.url == _url.text.trim()) {
        _key.clear();
        _editing = null;
      }
    }
  }

  @override
  void dispose() {
    for (final controller in <TextEditingController>[
      _name,
      _url,
      _model,
      _keyName,
      _key,
      _input,
      _output,
      _image,
      _references,
    ]) {
      controller.dispose();
    }
    super.dispose();
  }

  void _open(KinetoConnection? connection) {
    setState(() {
      _editing = connection?.name ?? '';
      _protocol = connection?.protocol ?? _protocols.first;
      _keyFollows = connection == null;
      _name.text = connection?.name ?? '';
      _url.text = connection?.url ?? '';
      _model.text = connection?.model ?? '';
      _keyName.text = connection?.key ?? '';
      _key.clear();
      _input.text = connection?.pricePerMillionInput ?? '';
      _output.text = connection?.pricePerMillionOutput ?? '';
      _image.text = connection?.pricePerImage ?? '';
      final references = connection?.references ?? 0;
      _references.text = references == 0 ? '' : '$references';
      _problem = null;
    });
  }

  void _save() {
    final text = widget.modality == KinetoModality.text;
    final references = _references.text.trim();
    final connection = KinetoConnection(
      name: _name.text.trim(),
      protocolName: _protocol.wireName,
      url: _url.text.trim(),
      model: _model.text.trim(),
      key: _keyName.text.trim(),
      pricePerMillionInput: text ? _input.text.trim() : '',
      pricePerMillionOutput: text ? _output.text.trim() : '',
      pricePerImage: text ? '' : _image.text.trim(),
      references: text || references.isEmpty ? 0 : int.tryParse(references) ?? -1,
    );
    final problem = _connectionProblem(connection);
    setState(() {
      _problem = problem;
      _sent = problem == null && widget.onPut != null;
    });
    if (problem == null) widget.onPut?.call(connection, _editing ?? '', _key.text.trim());
  }

  String _keyLabel(KinetoConnection connection) {
    if (connection.key.isEmpty) return 'Takes no key';
    final state = widget.keys?.states[connection.key];
    if (state == null) return 'Key ${connection.key}';
    if (state.stored) return 'Key ${connection.key} · saved';
    if (state.inEnvironment) return 'Key ${connection.key} · from the environment';
    return 'Key ${connection.key} · not saved';
  }

  Widget _row(KinetoConnection connection, bool ready) {
    final protocol = connection.protocol;
    return WabPanel(
      key: ValueKey<String>('connection-${connection.name}'),
      title: '${connection.name} · ${protocol?.label ?? connection.protocolName}',
      trailing: protocol == null
          ? const WabStatusBadge('Newer version', kind: WabBadgeKind.warning)
          : WabStatusBadge(
              _keyLabel(connection).endsWith('not saved') ? 'Needs its key' : 'Ready',
              kind: _keyLabel(connection).endsWith('not saved') ? WabBadgeKind.warning : WabBadgeKind.neutral,
            ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          SelectableText('${connection.model} at ${connection.url}'),
          Text(_keyLabel(connection)),
          Text(_priceLabel(connection)),
          if (connection.protocol?.modality == KinetoModality.image)
            Text(connection.references > 0
                ? 'Takes ${connection.references} reference images'
                : 'Takes no reference images, so it cannot lock a cast'),
          if (connection.requestFields.isNotEmpty)
            Text('Also sends ${connection.requestFields}, as the settings file says'),
          if (protocol == null) Text('A newer version of Kineto added it; this one cannot use it.'),
          const SizedBox(height: 8),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: <Widget>[
              KinetoButton(
                key: ValueKey<String>('connection-edit-${connection.name}'),
                onPressed: ready && protocol != null ? () => _open(connection) : null,
                child: const Text('Change'),
              ),
              KinetoButton(
                key: ValueKey<String>('connection-remove-${connection.name}'),
                onPressed: ready && widget.onRemove != null ? () => widget.onRemove!(connection) : null,
                child: const Text('Remove'),
              ),
            ],
          ),
        ],
      ),
    );
  }

  Widget _field(String key, TextEditingController controller, String label,
          {bool enabled = true, bool secret = false, ValueChanged<String>? onChanged}) =>
      TextField(
        key: ValueKey<String>('connection-$key'),
        controller: controller,
        enabled: enabled,
        obscureText: secret,
        enableSuggestions: !secret,
        autocorrect: false,
        onChanged: onChanged,
        decoration: InputDecoration(labelText: label),
      );

  Widget _form(bool ready) {
    final text = widget.modality == KinetoModality.text;
    final canKeep = widget.keys?.available ?? false;
    return WabPanel(
      key: const ValueKey<String>('connection-form'),
      title: _editing!.isEmpty ? 'New ${widget.modality.noun} connection' : 'Change $_editing',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          _field('name', _name, 'Name, such as deepseek: lowercase letters, digits and _', enabled: ready),
          const SizedBox(height: 8),
          Row(
            children: <Widget>[
              const SizedBox(width: 120, child: Text('Protocol')),
              Expanded(
                child: DropdownButton<KinetoProtocol>(
                  key: const ValueKey<String>('connection-protocol'),
                  value: _protocol,
                  isExpanded: true,
                  items: <DropdownMenuItem<KinetoProtocol>>[
                    for (final protocol in _protocols)
                      DropdownMenuItem<KinetoProtocol>(
                        key: ValueKey<String>('connection-protocol-${protocol.wireName}'),
                        value: protocol,
                        child: Text(protocol.label),
                      ),
                  ],
                  onChanged: ready ? (protocol) => setState(() => _protocol = protocol ?? _protocol) : null,
                ),
              ),
            ],
          ),
          Text('Spoken by ${_protocol.spokenBy}.'),
          _field('url', _url, 'Address, such as ${_protocol.exampleUrl}', enabled: ready),
          _field('model', _model, 'Model, as the service names it', enabled: ready),
          _field(
            'key-name',
            _keyName,
            'Name its key is kept under; empty for a service that takes none',
            enabled: ready,
            onChanged: (_) => _keyFollows = false,
          ),
          if (canKeep)
            _field(
              'key',
              _key,
              _editing!.isEmpty ? 'Its key, kept in secure storage' : 'A new key; empty keeps the saved one',
              enabled: ready,
              secret: true,
            ),
          if (text) ...<Widget>[
            _field('price-input', _input, 'US dollars per million tokens read; empty if you do not know',
                enabled: ready),
            _field('price-output', _output, 'US dollars per million tokens written; empty if you do not know',
                enabled: ready),
          ] else ...<Widget>[
            _field('price-image', _image, 'US dollars an image; empty if you do not know', enabled: ready),
            _field('references', _references, 'Reference images it takes, 0 to 16; empty for none',
                enabled: ready),
          ],
          if (_problem != null) ...<Widget>[
            const SizedBox(height: 8),
            Text(_problem!, key: const ValueKey<String>('connection-problem')),
          ],
          const SizedBox(height: 12),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: <Widget>[
              KinetoButton(
                key: const ValueKey<String>('connection-save'),
                primary: true,
                onPressed: ready ? _save : null,
                child: const Text('Save the connection'),
              ),
              KinetoButton(
                key: const ValueKey<String>('connection-cancel'),
                onPressed: widget.working ? null : () => setState(() => _editing = null),
                child: const Text('Cancel'),
              ),
            ],
          ),
        ],
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final settings = widget.settings;
    final connections = settings?.connectionsFor(widget.modality) ?? const <KinetoConnection>[];
    final unknown = widget.modality == KinetoModality.text
        ? <KinetoConnection>[
            for (final connection in settings?.connections ?? const <KinetoConnection>[])
              if (connection.protocol == null) connection,
          ]
        : const <KinetoConnection>[];
    final ready = settings != null && settings.problem == null && !widget.working && widget.onPut != null;
    return WabPanel(
      title: '${widget.modality.noun[0].toUpperCase()}${widget.modality.noun.substring(1)} connections',
      trailing: WabStatusBadge('${connections.length}', kind: WabBadgeKind.neutral),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          Text(
            'Any service for ${widget.modality.usedFor}, added by the protocol it speaks: its address, '
            'its model and its key. Choose it above once it is saved.',
          ),
          for (final connection in <KinetoConnection>[...connections, ...unknown]) ...<Widget>[
            const SizedBox(height: 12),
            _row(connection, ready && _editing == null),
          ],
          const SizedBox(height: 12),
          if (_editing != null)
            _form(ready)
          else
            Align(
              alignment: Alignment.centerLeft,
              child: KinetoButton(
                key: ValueKey<String>('connection-add-${widget.modality.name}'),
                onPressed: ready ? () => _open(null) : null,
                child: Text('Add ${widget.modality.withArticle} connection'),
              ),
            ),
        ],
      ),
    );
  }
}
