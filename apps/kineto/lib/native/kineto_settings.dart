part of 'kineto_engine.dart';

@Native<Int32 Function(Pointer<Pointer<Void>>)>(symbol: 'kineto_settings_load')
external int _kinetoSettingsLoad(Pointer<Pointer<Void>> outSettings);

@Native<Int32 Function(Pointer<Void>, Uint32, Pointer<Uint8>, Uint64, Pointer<Uint64>)>(
  symbol: 'kineto_settings_text_copy',
)
external int _kinetoSettingsTextCopy(
  Pointer<Void> settings,
  int field,
  Pointer<Uint8> buffer,
  int capacity,
  Pointer<Uint64> length,
);

@Native<Uint8 Function(Uint32)>(symbol: 'kineto_settings_overridden')
external int _kinetoSettingsOverridden(int field);

@Native<Int32 Function(Pointer<Void>, Pointer<Uint8>, Uint64, Pointer<Uint64>)>(
  symbol: 'kineto_settings_problem_copy',
)
external int _kinetoSettingsProblemCopy(
  Pointer<Void> settings,
  Pointer<Uint8> buffer,
  int capacity,
  Pointer<Uint64> length,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Pointer<Uint8>, Uint64, Pointer<Uint64>)>(
  symbol: 'kineto_settings_agent_path_copy',
)
external int _kinetoSettingsAgentPathCopy(
  Pointer<Void> settings,
  int agent,
  Pointer<Uint8> buffer,
  int capacity,
  Pointer<Uint64> length,
);

@Native<Void Function(Pointer<Void>)>(symbol: 'kineto_settings_destroy')
external void _kinetoSettingsDestroy(Pointer<Void> settings);

@Native<Int32 Function(Uint32, Pointer<Uint8>, Uint64)>(symbol: 'kineto_settings_set')
external int _kinetoSettingsSet(int field, Pointer<Uint8> value, int length);

/// One of the director's provider choices.
enum KinetoProviderSetting {
  image('image', 'KINETO_IMAGE_PROVIDER'),
  video('video', 'KINETO_VIDEO_PROVIDER'),
  speech('speech', 'KINETO_SPEECH_PROVIDER'),
  languageModel('language_model', 'KINETO_LLM_PROVIDER'),
  endpointUrl('endpoint_url', 'KINETO_LOCAL_LLM_URL'),
  endpointModel('endpoint_model', 'KINETO_LOCAL_LLM_MODEL'),
  endpointKey('endpoint_key', 'KINETO_LOCAL_LLM_KEY');

  const KinetoProviderSetting(this.wireName, this.variable);

  final String wireName;

  /// The environment variable that makes this choice for a run instead.
  final String variable;
}

/// An agent command-line tool the director may have installed and signed in
/// to, which Kineto can drive.
enum KinetoAgent {
  claudeCode('claude_code', 'claude-code', 'Claude Code'),
  codex('codex', 'codex', 'Codex'),
  kimiCode(
    'kimi_code',
    'kimi-code',
    'Kimi Code',
    caveat: 'Kimi Code runs your own hooks with each request, and takes a request of at most '
        'about 120 KB (28 KB on Windows).',
  ),
  agy(
    'agy',
    'agy',
    'Antigravity (agy)',
    makesImages: true,
    caveat: 'agy runs your own hooks with each request.',
  ),
  miniAgent(
    'mini_agent',
    'mini-agent',
    'Mini-Agent',
    caveat: 'Mini-Agent uses the MiniMax key in its own configuration, charged per request.',
  );

  const KinetoAgent(this.wireName, this.selection, this.label, {this.makesImages = false, this.caveat});

  final String wireName;

  /// How the provider settings name it.
  final String selection;
  final String label;
  final bool makesImages;

  /// What Kineto cannot turn off when it drives it.
  final String? caveat;
}

/// The director's provider choices and the agent tools installed.
final class KinetoProviderSettings {
  const KinetoProviderSettings({
    this.values = const <KinetoProviderSetting, String>{},
    this.overridden = const <KinetoProviderSetting>{},
    this.agents = const <KinetoAgent, String>{},
    this.problem,
  });

  /// Each choice made; a missing one is not made.
  final Map<KinetoProviderSetting, String> values;

  /// Choices an environment variable makes for this run instead.
  final Set<KinetoProviderSetting> overridden;

  /// Where each installed agent was found; a missing one was not.
  final Map<KinetoAgent, String> agents;

  /// Why the settings file cannot be read; null when it can.
  final String? problem;

  String value(KinetoProviderSetting setting) => values[setting] ?? '';
}

/// Read the provider settings and look for each agent tool. Reads a file and
/// looks for programs, so call it off the UI thread, as
/// [loadKinetoProviderSettingsInBackground] does.
KinetoProviderSettings loadKinetoProviderSettings() {
  _assertAbiCompatible();
  final contract = KinetoNativeContract.instance;
  final out = calloc<Pointer<Void>>();
  try {
    _checkProjectResult(_kinetoSettingsLoad(out));
    final handle = out.value;
    try {
      final values = <KinetoProviderSetting, String>{};
      final overridden = <KinetoProviderSetting>{};
      for (final setting in KinetoProviderSetting.values) {
        final code = contract.code(contract.providerSettings, setting.wireName);
        final value = _copyProjectUtf8(
          handle,
          (settings, buffer, capacity, length) => _kinetoSettingsTextCopy(settings, code, buffer, capacity, length),
          'setting ${setting.wireName}',
        );
        if (value.isNotEmpty) values[setting] = value;
        if (_kinetoSettingsOverridden(code) != 0) overridden.add(setting);
      }
      final agents = <KinetoAgent, String>{};
      for (final agent in KinetoAgent.values) {
        final code = contract.code(contract.agents, agent.wireName);
        final path = _copyProjectUtf8(
          handle,
          (settings, buffer, capacity, length) =>
              _kinetoSettingsAgentPathCopy(settings, code, buffer, capacity, length),
          'agent ${agent.wireName}',
        );
        if (path.isNotEmpty) agents[agent] = path;
      }
      final problem = _copyProjectUtf8(handle, _kinetoSettingsProblemCopy, 'settings problem');
      return KinetoProviderSettings(
        values: values,
        overridden: overridden,
        agents: agents,
        problem: problem.isEmpty ? null : problem,
      );
    } finally {
      _kinetoSettingsDestroy(handle);
    }
  } finally {
    calloc.free(out);
  }
}

/// Choose [value] for [setting] and save the settings; empty clears the
/// choice. Projects opened after it use it.
void setKinetoProviderSetting(KinetoProviderSetting setting, String value) {
  _assertAbiCompatible();
  final contract = KinetoNativeContract.instance;
  final code = contract.code(contract.providerSettings, setting.wireName);
  _withUtf8(value.trim(), (bytes, length) => _checkProjectResult(_kinetoSettingsSet(code, bytes, length)));
}

/// [loadKinetoProviderSettings] on another isolate.
Future<KinetoProviderSettings> loadKinetoProviderSettingsInBackground() => Isolate.run(loadKinetoProviderSettings);

/// [setKinetoProviderSetting] on another isolate.
Future<void> setKinetoProviderSettingInBackground(KinetoProviderSetting setting, String value) =>
    Isolate.run(() => setKinetoProviderSetting(setting, value));
