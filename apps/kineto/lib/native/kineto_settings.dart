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

@Native<Uint64 Function(Pointer<Void>)>(symbol: 'kineto_settings_connection_count')
external int _kinetoSettingsConnectionCount(Pointer<Void> settings);

@Native<Int32 Function(Pointer<Void>, Uint64, Uint32, Pointer<Uint8>, Uint64, Pointer<Uint64>)>(
  symbol: 'kineto_settings_connection_text_copy',
)
external int _kinetoSettingsConnectionTextCopy(
  Pointer<Void> settings,
  int index,
  int field,
  Pointer<Uint8> buffer,
  int capacity,
  Pointer<Uint64> length,
);

/// One field of a connection, laid out as `KinetoConnectionField`.
final class _NativeConnectionField extends Struct {
  @Uint32()
  external int field;

  external _NativeText value;
}

@Native<Int32 Function(Pointer<Uint8>, Uint64, Pointer<_NativeConnectionField>, Uint64)>(
  symbol: 'kineto_settings_connection_put',
)
external int _kinetoSettingsConnectionPut(
  Pointer<Uint8> previous,
  int previousLength,
  Pointer<_NativeConnectionField> fields,
  int fieldCount,
);

@Native<Int32 Function(Pointer<Uint8>, Uint64)>(symbol: 'kineto_settings_connection_remove')
external int _kinetoSettingsConnectionRemove(Pointer<Uint8> name, int length);

/// One of the director's provider choices.
enum KinetoProviderSetting {
  image('image', 'KINETO_IMAGE_PROVIDER'),
  video('video', 'KINETO_VIDEO_PROVIDER'),
  speech('speech', 'KINETO_SPEECH_PROVIDER'),
  languageModel('language_model', 'KINETO_LLM_PROVIDER');

  const KinetoProviderSetting(this.wireName, this.variable);

  final String wireName;

  /// The environment variable that makes this choice for a run instead.
  final String variable;
}

/// What a connection makes.
enum KinetoModality {
  text('text', 'documents'),
  image('image', 'keyframes, style samples and portraits'),
  video('video', "shots' clips");

  const KinetoModality(this.noun, this.usedFor);

  /// What it makes, as a word before "connection".
  final String noun;
  final String usedFor;

  /// `a text`, `an image`.
  String get withArticle => '${RegExp('^[aeiou]').hasMatch(noun) ? 'an' : 'a'} $noun';
}

/// A protocol a connection may speak: how its requests and answers are
/// shaped, which many services share. Never a vendor.
enum KinetoProtocol {
  openAiChat(
    'openai-chat',
    KinetoModality.text,
    'OpenAI chat completions',
    'GPT, Grok, Muse, GLM, Qwen, DeepSeek, Kimi, Doubao, MiniMax, Gemini, OpenRouter, Ollama and most others',
    'https://api.deepseek.com/v1',
  ),
  anthropicMessages(
    'anthropic-messages',
    KinetoModality.text,
    'Anthropic messages',
    "Claude's API, and the Anthropic-compatible addresses other services give",
    'https://api.anthropic.com',
  ),
  openAiImages(
    'openai-images',
    KinetoModality.image,
    'OpenAI images',
    'GPT Image, Grok Imagine, Seedream, CogView and others that take /images/generations',
    'https://api.openai.com/v1',
  ),
  geminiImages(
    'gemini-images',
    KinetoModality.image,
    'Gemini image generation',
    "Gemini's image models, by generateContent",
    'https://generativelanguage.googleapis.com/v1beta',
  ),
  arkVideo(
    'ark-video',
    KinetoModality.video,
    'Ark video tasks (Seedance)',
    'Seedance, on Volcengine Ark, BytePlus ModelArk and the gateways that resell it',
    'https://ark.ap-southeast.bytepluses.com/api/v3',
  );

  const KinetoProtocol(this.wireName, this.modality, this.label, this.spokenBy, this.exampleUrl);

  /// Its name in the settings file.
  final String wireName;
  final KinetoModality modality;
  final String label;

  /// Who speaks it, as examples.
  final String spokenBy;
  final String exampleUrl;

  /// Its name in the native contract table.
  String get contractName => wireName.replaceAll('-', '_');

  static KinetoProtocol? byWireName(String name) =>
      KinetoProtocol.values.where((protocol) => protocol.wireName == name).firstOrNull;
}

/// A service the director added by the protocol it speaks.
final class KinetoConnection {
  const KinetoConnection({
    required this.name,
    required this.protocolName,
    required this.url,
    required this.model,
    this.key = '',
    this.pricePerMillionInput = '',
    this.pricePerMillionOutput = '',
    this.pricePerImage = '',
    this.pricePerSecond = '',
    this.references = 0,
    this.requestFields = '',
  });

  /// How it is chosen, and the key's name unless it names another.
  final String name;

  /// Its protocol's name; one a newer build added has no [protocol].
  final String protocolName;
  final String url;
  final String model;

  /// The name its key is kept under; empty for a service that takes none.
  final String key;

  /// Prices in US dollars, as written; empty when not given.
  final String pricePerMillionInput;
  final String pricePerMillionOutput;
  final String pricePerImage;
  final String pricePerSecond;

  /// How many reference images an image connection takes.
  final int references;

  /// The names of the fields the settings file adds to each request, as
  /// the file has them; settings never change them.
  final String requestFields;

  KinetoProtocol? get protocol => KinetoProtocol.byWireName(protocolName);

  /// How the provider settings choose it.
  String get selection => 'connection:$name';

  Map<String, String> get _fields => <String, String>{
        'name': name,
        'protocol': protocolName,
        'url': url,
        'model': model,
        'key': key,
        'price_per_million_input': pricePerMillionInput,
        'price_per_million_output': pricePerMillionOutput,
        'price_per_image': pricePerImage,
        'price_per_second': pricePerSecond,
        'references': references == 0 ? '' : '$references',
      };
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
    this.connections = const <KinetoConnection>[],
    this.problem,
  });

  /// Each choice made; a missing one is not made.
  final Map<KinetoProviderSetting, String> values;

  /// Choices an environment variable makes for this run instead.
  final Set<KinetoProviderSetting> overridden;

  /// Where each installed agent was found; a missing one was not.
  final Map<KinetoAgent, String> agents;

  /// The services the director added, in the order they were added.
  final List<KinetoConnection> connections;

  /// Why the settings file cannot be read; null when it can.
  final String? problem;

  String value(KinetoProviderSetting setting) => values[setting] ?? '';

  /// The connections that make [modality].
  List<KinetoConnection> connectionsFor(KinetoModality modality) => <KinetoConnection>[
        for (final connection in connections)
          if (connection.protocol?.modality == modality) connection,
      ];
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
      final connections = <KinetoConnection>[];
      for (var index = 0; index < _kinetoSettingsConnectionCount(handle); index++) {
        String field(String name) {
          final code = contract.code(contract.connectionFields, name);
          return _copyProjectUtf8(
            handle,
            (settings, buffer, capacity, length) =>
                _kinetoSettingsConnectionTextCopy(settings, index, code, buffer, capacity, length),
            'connection $name',
          );
        }

        connections.add(KinetoConnection(
          name: field('name'),
          protocolName: field('protocol'),
          url: field('url'),
          model: field('model'),
          key: field('key'),
          pricePerMillionInput: field('price_per_million_input'),
          pricePerMillionOutput: field('price_per_million_output'),
          pricePerImage: field('price_per_image'),
          pricePerSecond: field('price_per_second'),
          references: int.tryParse(field('references')) ?? 0,
          requestFields: field('request_fields'),
        ));
      }
      final problem = _copyProjectUtf8(handle, _kinetoSettingsProblemCopy, 'settings problem');
      return KinetoProviderSettings(
        values: values,
        overridden: overridden,
        agents: agents,
        connections: connections,
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

/// Add [connection], or put it in place of the one named [previous], and
/// save the settings; a choice of the one replaced follows it. Projects
/// opened after it use it. Refused when the connection cannot be used as it
/// is.
void putKinetoConnection(KinetoConnection connection, {String previous = ''}) {
  _assertAbiCompatible();
  final contract = KinetoNativeContract.instance;
  final arena = Arena();
  try {
    final fields = connection._fields;
    final native = arena<_NativeConnectionField>(fields.length);
    var index = 0;
    for (final MapEntry(key: name, :value) in fields.entries) {
      final text = _encodeUtf8(arena, value.trim());
      native[index]
        ..field = contract.code(contract.connectionFields, name)
        ..value.pointer = text.pointer
        ..value.length = text.length;
      index++;
    }
    final replaced = _encodeUtf8(arena, previous);
    _checkProjectResult(_kinetoSettingsConnectionPut(replaced.pointer, replaced.length, native, fields.length));
  } finally {
    arena.releaseAll();
  }
}

/// Remove the connection named [name] and any choice of it, and save the
/// settings. Its key stays where it is kept.
void removeKinetoConnection(String name) {
  _assertAbiCompatible();
  _withUtf8(name.trim(), (bytes, length) => _checkProjectResult(_kinetoSettingsConnectionRemove(bytes, length)));
}

/// [putKinetoConnection] on another isolate.
Future<void> putKinetoConnectionInBackground(KinetoConnection connection, {String previous = ''}) =>
    Isolate.run(() => putKinetoConnection(connection, previous: previous));

/// [removeKinetoConnection] on another isolate.
Future<void> removeKinetoConnectionInBackground(String name) => Isolate.run(() => removeKinetoConnection(name));

/// [loadKinetoProviderSettings] on another isolate.
Future<KinetoProviderSettings> loadKinetoProviderSettingsInBackground() => Isolate.run(loadKinetoProviderSettings);

/// [setKinetoProviderSetting] on another isolate.
Future<void> setKinetoProviderSettingInBackground(KinetoProviderSetting setting, String value) =>
    Isolate.run(() => setKinetoProviderSetting(setting, value));
