part of 'kineto_engine.dart';

@Native<Int32 Function(Pointer<Uint8>, Uint64, Pointer<Uint64>)>(symbol: 'kineto_keys_unavailable_copy')
external int _kinetoKeysUnavailableCopy(Pointer<Uint8> buffer, int capacity, Pointer<Uint64> length);

@Native<Int32 Function(Pointer<Uint8>, Uint64, Pointer<Uint32>)>(symbol: 'kineto_keys_state')
external int _kinetoKeysState(Pointer<Uint8> name, int nameLength, Pointer<Uint32> outState);

@Native<Int32 Function(Pointer<Uint8>, Uint64, Pointer<Uint8>, Uint64)>(symbol: 'kineto_keys_store')
external int _kinetoKeysStore(Pointer<Uint8> name, int nameLength, Pointer<Uint8> key, int length);

@Native<Int32 Function(Pointer<Uint8>, Uint64)>(symbol: 'kineto_keys_delete')
external int _kinetoKeysDelete(Pointer<Uint8> name, int nameLength);

/// A key settings manage, by the name it is kept under in the system's
/// secure storage: a built-in provider's, the one an OpenAI-compatible
/// server takes, or a secret a plugin's or MCP server's entry lists.
final class KinetoKeySlot {
  const KinetoKeySlot({required this.name, required this.label, required this.usedFor});

  /// The key the OpenAI-compatible server in the provider settings takes,
  /// under the name the director gave it.
  const KinetoKeySlot.server(this.name)
      : label = name,
        usedFor = 'The OpenAI-compatible server chosen for documents';

  /// A secret the entry of [owner] (a plugin or MCP server) lists.
  KinetoKeySlot.secret(this.name, String owner)
      : label = name,
        usedFor = 'Given to $owner';

  static const KinetoKeySlot minimax = KinetoKeySlot(
    name: 'minimax',
    label: 'MiniMax',
    usedFor: 'Images, Hailuo video and speech',
  );
  static const KinetoKeySlot gemini = KinetoKeySlot(
    name: 'gemini',
    label: 'Gemini API',
    usedFor: 'Gemini Omni Flash video',
  );
  static const KinetoKeySlot kimi = KinetoKeySlot(
    name: 'kimi',
    label: 'Kimi API',
    usedFor: 'Documents, when the language model is the Kimi API',
  );
  static const KinetoKeySlot veo = KinetoKeySlot(
    name: 'veo',
    label: 'Google Veo',
    usedFor: 'Veo 3.1 video, which Google retires on 2026-10-22',
  );

  /// The built-in providers' keys, as the engine lists them.
  static const List<KinetoKeySlot> builtIn = <KinetoKeySlot>[minimax, gemini, kimi, veo];

  /// The name it is kept under.
  final String name;
  final String label;
  final String usedFor;

  /// The environment variable read when the secure storage holds none: a
  /// provider's lowercase name reads `KINETO_<NAME>_API_KEY`, a secret its
  /// own name.
  String get variable => name == name.toLowerCase() ? 'KINETO_${name.toUpperCase()}_API_KEY' : name;

  @override
  bool operator ==(Object other) => other is KinetoKeySlot && other.name == name;

  @override
  int get hashCode => name.hashCode;
}

/// Where one key is.
final class KinetoKeyState {
  const KinetoKeyState({required this.stored, required this.inEnvironment});

  /// In the system's secure storage, which is read first.
  final bool stored;

  /// In the environment, read when the secure storage holds none.
  final bool inEnvironment;
}

/// The system's secure storage and where each key is, by name.
final class KinetoKeys {
  const KinetoKeys({required this.unavailable, required this.states, this.problems = const {}});

  /// Why there is no secure storage, or null when there is one.
  final String? unavailable;
  final Map<String, KinetoKeyState> states;

  /// Why a key could not be read, as when the storage is locked.
  final Map<String, String> problems;

  bool get available => unavailable == null;
}

/// [body] with [text] as UTF-8 in native memory, cleared and freed after.
T _withUtf8<T>(String text, T Function(Pointer<Uint8> bytes, int length) body) {
  final bytes = utf8.encode(text);
  final size = bytes.isEmpty ? 1 : bytes.length;
  final buffer = calloc<Uint8>(size);
  try {
    buffer.asTypedList(bytes.length).setAll(0, bytes);
    return body(buffer, bytes.length);
  } finally {
    buffer.asTypedList(size).fillRange(0, size, 0);
    bytes.fillRange(0, bytes.length, 0);
    calloc.free(buffer);
  }
}

/// Read the secure storage's state and where each of [names] is. Opening the
/// storage can wait on the user or a locked keyring, so call it off the UI
/// thread, as [loadKinetoKeysInBackground] does.
KinetoKeys loadKinetoKeys(List<String> names) {
  _assertAbiCompatible();
  final contract = KinetoNativeContract.instance;
  final unavailable = _copyProjectUtf8(
    nullptr,
    (_, buffer, capacity, length) => _kinetoKeysUnavailableCopy(buffer, capacity, length),
    'keys unavailable',
  );
  final stored = contract.code(contract.keyStates, 'stored');
  final environment = contract.code(contract.keyStates, 'environment');
  final states = <String, KinetoKeyState>{};
  final problems = <String, String>{};
  final out = calloc<Uint32>();
  try {
    for (final name in names) {
      final code = _withUtf8(name, (bytes, length) => _kinetoKeysState(bytes, length, out));
      if (code != 0) {
        problems[name] = _projectException(code).message;
        continue;
      }
      states[name] = KinetoKeyState(
        stored: out.value & stored != 0,
        inEnvironment: out.value & environment != 0,
      );
    }
  } finally {
    calloc.free(out);
  }
  return KinetoKeys(
    unavailable: unavailable.isEmpty ? null : unavailable,
    states: states,
    problems: problems,
  );
}

/// Store [key] under [name] in the system's secure storage. The bytes handed
/// to the engine are cleared once it has them. Takes effect for projects
/// opened after it.
void storeKinetoKey(String name, String key) {
  _assertAbiCompatible();
  _withUtf8(name, (nameBytes, nameLength) {
    _withUtf8(key.trim(), (keyBytes, keyLength) {
      _checkProjectResult(_kinetoKeysStore(nameBytes, nameLength, keyBytes, keyLength));
    });
  });
}

/// Remove the key named [name] from the system's secure storage.
void deleteKinetoKey(String name) {
  _assertAbiCompatible();
  _withUtf8(name, (bytes, length) => _checkProjectResult(_kinetoKeysDelete(bytes, length)));
}

/// [loadKinetoKeys] on another isolate.
Future<KinetoKeys> loadKinetoKeysInBackground(List<String> names) => Isolate.run(() => loadKinetoKeys(names));

/// [storeKinetoKey] on another isolate.
Future<void> storeKinetoKeyInBackground(String name, String key) => Isolate.run(() => storeKinetoKey(name, key));

/// [deleteKinetoKey] on another isolate.
Future<void> deleteKinetoKeyInBackground(String name) => Isolate.run(() => deleteKinetoKey(name));
