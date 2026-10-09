import 'package:flutter/material.dart';
import 'package:wabisabi/wabisabi.dart';

import 'native/kineto_engine.dart';
import 'screenplay_workspace.dart';

/// Which providers make each part of a film, the keys they take, kept in
/// the system's secure storage, and the provider plugins and MCP servers the
/// user registered, read only: both are registered in Kineto's configuration
/// folder, never through the app, and this page never asks for a program to
/// run.
class SettingsWorkspaceScreen extends StatefulWidget {
  const SettingsWorkspaceScreen({
    this.loadSettings = loadKinetoProviderSettingsInBackground,
    this.setSetting = setKinetoProviderSettingInBackground,
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

  /// Reads the provider choices and looks for agent tools; changes one.
  /// Both touch files, so they run off the UI thread.
  final Future<KinetoProviderSettings> Function() loadSettings;
  final Future<void> Function(KinetoProviderSetting setting, String value) setSetting;

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
  final Widget? navigation;
  final bool isDarkMode;
  final VoidCallback? onThemeToggle;

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

  /// The keys settings list: the built-in providers', the one the chosen
  /// OpenAI-compatible server takes, and every secret a plugin or MCP
  /// server entry lists.
  List<KinetoKeySlot> get _slots {
    final slots = <KinetoKeySlot>[...KinetoKeySlot.builtIn];
    void add(KinetoKeySlot slot) {
      if (!slots.contains(slot)) slots.add(slot);
    }

    final server = _settings?.value(KinetoProviderSetting.endpointKey) ?? '';
    if (server.isNotEmpty) add(KinetoKeySlot.server(server));
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
    String note = 'Saved. Projects opened from now on use it.';
    try {
      for (final MapEntry(key: setting, value: value) in choices.entries) {
        await widget.setSetting(setting, value);
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
        slots: _slots,
        keys: _keys,
        keysProblem: _keysProblem,
        keyWorking: _keyWorking,
        keyNote: _keyNote,
        onStoreKey: (slot, key) => _changeKey(
          slot.name,
          () => widget.storeKey(slot.name, key),
          '${slot.label} key saved. Projects opened from now on use it.',
        ),
        onDeleteKey: (slot) => _changeKey(
          slot.name,
          () => widget.deleteKey(slot.name),
          '${slot.label} key removed. Projects opened from now on do without it.',
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
    this.settings,
    this.settingsProblem,
    this.settingWorking = false,
    this.settingNote,
    this.onChoose,
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
    this.navigation,
    this.isDarkMode = true,
    this.onThemeToggle,
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
                  _ProvidersPanel(
                    key: const ValueKey<String>('providers'),
                    settings: settings,
                    problem: settingsProblem,
                    working: settingWorking,
                    note: settingNote,
                    plugins: plugins ?? const <KinetoPlugin>[],
                    onChoose: onChoose,
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
                WabButton(
                  key: ValueKey<String>('key-save-${slot.name}'),
                  kind: WabMaterialKind.seal,
                  onPressed: ready ? _save : null,
                  child: Text(stored ? 'Replace' : 'Save'),
                ),
                if (stored) ...<Widget>[
                  const SizedBox(width: 12),
                  WabButton(
                    key: ValueKey<String>('key-delete-${slot.name}'),
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

/// One thing a provider setting can be, as the director reads it.
typedef _Option = ({String value, String label, bool enabled});

/// Which provider makes each part of a film, and the server documents are
/// written with when it is an OpenAI-compatible one.
class _ProvidersPanel extends StatefulWidget {
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

  @override
  State<_ProvidersPanel> createState() => _ProvidersPanelState();
}

class _ProvidersPanelState extends State<_ProvidersPanel> {
  final TextEditingController _url = TextEditingController();
  final TextEditingController _model = TextEditingController();
  final TextEditingController _keyName = TextEditingController();

  @override
  void initState() {
    super.initState();
    _fill();
  }

  @override
  void didUpdateWidget(_ProvidersPanel old) {
    super.didUpdateWidget(old);
    if (!identical(old.settings, widget.settings)) _fill();
  }

  void _fill() {
    final settings = widget.settings;
    if (settings == null) return;
    _url.text = settings.value(KinetoProviderSetting.endpointUrl);
    _model.text = settings.value(KinetoProviderSetting.endpointModel);
    _keyName.text = settings.value(KinetoProviderSetting.endpointKey);
  }

  @override
  void dispose() {
    _url.dispose();
    _model.dispose();
    _keyName.dispose();
    super.dispose();
  }

  String _agentLabel(KinetoAgent agent) =>
      '${agent.label} · ${widget.settings?.agents.containsKey(agent) ?? false ? 'installed' : 'not found'}';

  List<_Option> _options(KinetoProviderSetting setting) {
    final agents = widget.settings?.agents ?? const <KinetoAgent, String>{};
    final current = widget.settings?.value(setting) ?? '';
    final plugins = <KinetoPlugin>[
      for (final plugin in widget.plugins)
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
            (value: 'openai-compatible', label: 'OpenAI-compatible server', enabled: true),
          ],
        KinetoProviderSetting.image => <_Option>[
            (value: '', label: 'Local demo · free', enabled: true),
            (value: 'minimax', label: 'MiniMax image-01 · key', enabled: true),
            for (final agent in KinetoAgent.values)
              if (agent.makesImages)
                (
                  value: agent.selection,
                  label: _agentLabel(agent),
                  enabled: agents.containsKey(agent) || current == agent.selection,
                ),
            for (final plugin in plugins)
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
            for (final plugin in plugins)
              if (plugin.description!.makesVideos)
                (value: plugin.selection, label: '${plugin.id} plugin', enabled: true),
          ],
        KinetoProviderSetting.speech => <_Option>[
            (value: '', label: 'Follows images', enabled: true),
            (value: 'local-demo', label: 'Local demo · free', enabled: true),
            (value: 'minimax', label: 'MiniMax speech · key', enabled: true),
            for (final plugin in plugins)
              if (plugin.description!.speaks)
                (value: plugin.selection, label: '${plugin.id} plugin', enabled: true),
          ],
        _ => const <_Option>[],
      },
    ];
    if (!options.any((option) => option.value == current)) {
      options.add((value: current, label: '$current (as set)', enabled: true));
    }
    return options;
  }

  Widget _choice(BuildContext context, KinetoProviderSetting setting, String title) {
    final settings = widget.settings!;
    final ready = widget.onChoose != null && !widget.working && settings.problem == null;
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
                          widget.onChoose!(<KinetoProviderSetting, String>{setting: value});
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
    final settings = widget.settings;
    final model = settings?.value(KinetoProviderSetting.languageModel) ?? '';
    final agent = KinetoAgent.values.where((agent) => agent.selection == model).firstOrNull;
    final server = model == 'openai-compatible' || model == 'local';
    final ready = widget.onChoose != null && !widget.working && settings?.problem == null;
    return WabPanel(
      title: 'Providers',
      trailing: settings == null
          ? WabStatusBadge(widget.problem == null ? 'Reading' : 'Unreadable',
              kind: widget.problem == null ? WabBadgeKind.progress : WabBadgeKind.warning)
          : widget.working
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
            'sign-in. Projects opened from now on use these choices.',
          ),
          if (widget.problem != null) ...<Widget>[const SizedBox(height: 12), Text(widget.problem!)],
          if (settings?.problem != null) ...<Widget>[
            const SizedBox(height: 12),
            Text(
              '${settings!.problem}. Nothing is changed here until the file can be read.',
              key: const ValueKey<String>('providers-unreadable'),
            ),
          ],
          if (widget.note != null) ...<Widget>[
            const SizedBox(height: 12),
            Text(widget.note!, key: const ValueKey<String>('providers-note')),
          ],
          if (settings != null) ...<Widget>[
            const SizedBox(height: 12),
            _choice(context, KinetoProviderSetting.languageModel, 'Documents'),
            if (agent != null && settings.agents[agent] != null)
              Text('Runs ${settings.agents[agent]}', key: const ValueKey<String>('agent-path')),
            if (agent?.caveat != null) Text(agent!.caveat!),
            if (server) ...<Widget>[
              const SizedBox(height: 8),
              TextField(
                key: const ValueKey<String>('server-url'),
                controller: _url,
                enabled: ready,
                decoration: const InputDecoration(labelText: 'Server address, https:// or http://localhost'),
              ),
              TextField(
                key: const ValueKey<String>('server-model'),
                controller: _model,
                enabled: ready,
                decoration: const InputDecoration(labelText: 'Model'),
              ),
              TextField(
                key: const ValueKey<String>('server-key'),
                controller: _keyName,
                enabled: ready,
                autocorrect: false,
                decoration: const InputDecoration(
                  labelText: 'Name of its key, such as deepseek; empty for a server that takes none',
                ),
              ),
              const SizedBox(height: 8),
              Align(
                alignment: Alignment.centerLeft,
                child: WabButton(
                  key: const ValueKey<String>('server-save'),
                  kind: WabMaterialKind.seal,
                  onPressed: ready
                      ? () => widget.onChoose!(<KinetoProviderSetting, String>{
                            KinetoProviderSetting.endpointUrl: _url.text,
                            KinetoProviderSetting.endpointModel: _model.text,
                            KinetoProviderSetting.endpointKey: _keyName.text,
                          })
                      : null,
                  child: const Text('Save the server'),
                ),
              ),
              const Text('Its key is kept under that name below, never here.'),
            ],
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
