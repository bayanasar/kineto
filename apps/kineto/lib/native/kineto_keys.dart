part of 'kineto_engine.dart';

@Native<Int32 Function(Pointer<Uint8>, Uint64, Pointer<Uint64>)>(symbol: 'kineto_keys_unavailable_copy')
external int _kinetoKeysUnavailableCopy(Pointer<Uint8> buffer, int capacity, Pointer<Uint64> length);

@Native<Int32 Function(Uint32, Pointer<Uint32>)>(symbol: 'kineto_keys_state')
external int _kinetoKeysState(int provider, Pointer<Uint32> outState);

@Native<Int32 Function(Uint32, Pointer<Uint8>, Uint64)>(symbol: 'kineto_keys_store')
external int _kinetoKeysStore(int provider, Pointer<Uint8> key, int length);

@Native<Int32 Function(Uint32)>(symbol: 'kineto_keys_delete')
external int _kinetoKeysDelete(int provider);

/// A provider whose key settings manage, by the name the engine stores it
/// under.
enum KinetoKeyProvider {
  minimax('minimax', 'MiniMax', 'Images, Hailuo video and speech'),
  gemini('gemini', 'Gemini API', 'Gemini Omni Flash video'),
  kimi('kimi', 'Kimi API', 'Documents, when KINETO_LLM_PROVIDER is kimi'),
  veo('veo', 'Google Veo', 'Veo 3.1 video, which Google retires on 2026-10-22');

  const KinetoKeyProvider(this.wireName, this.label, this.usedFor);

  final String wireName;
  final String label;
  final String usedFor;

  /// The environment variable read when the secure storage holds no key.
  String get variable => 'KINETO_${wireName.toUpperCase()}_API_KEY';
}

/// Where one provider's key is.
final class KinetoKeyState {
  const KinetoKeyState({required this.stored, required this.inEnvironment});

  /// In the system's secure storage, which is read first.
  final bool stored;

  /// In the environment, read when the secure storage holds none.
  final bool inEnvironment;
}

/// The system's secure storage and where each provider's key is.
final class KinetoKeys {
  const KinetoKeys({required this.unavailable, required this.states, this.problems = const {}});

  /// Why there is no secure storage, or null when there is one.
  final String? unavailable;
  final Map<KinetoKeyProvider, KinetoKeyState> states;

  /// Why a provider's key could not be read, as when the storage is locked.
  final Map<KinetoKeyProvider, String> problems;

  bool get available => unavailable == null;
}

/// Read the secure storage's state and each provider's. Opening the storage
/// can wait on the user or a locked keyring, so call it off the UI thread,
/// as [loadKinetoKeysInBackground] does.
KinetoKeys loadKinetoKeys() {
  _assertAbiCompatible();
  final contract = KinetoNativeContract.instance;
  final unavailable = _copyProjectUtf8(
    nullptr,
    (_, buffer, capacity, length) => _kinetoKeysUnavailableCopy(buffer, capacity, length),
    'keys unavailable',
  );
  final stored = contract.code(contract.keyStates, 'stored');
  final environment = contract.code(contract.keyStates, 'environment');
  final states = <KinetoKeyProvider, KinetoKeyState>{};
  final problems = <KinetoKeyProvider, String>{};
  final out = calloc<Uint32>();
  try {
    for (final provider in KinetoKeyProvider.values) {
      final code = _kinetoKeysState(contract.code(contract.keyProviders, provider.wireName), out);
      if (code != 0) {
        problems[provider] = _projectException(code).message;
        continue;
      }
      states[provider] = KinetoKeyState(
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

/// Store [key] as [provider]'s in the system's secure storage. The bytes
/// handed to the engine are cleared once it has them. Takes effect for
/// projects opened after it.
void storeKinetoKey(KinetoKeyProvider provider, String key) {
  _assertAbiCompatible();
  final contract = KinetoNativeContract.instance;
  final bytes = utf8.encode(key.trim());
  final buffer = calloc<Uint8>(bytes.isEmpty ? 1 : bytes.length);
  try {
    buffer.asTypedList(bytes.length).setAll(0, bytes);
    _checkProjectResult(
      _kinetoKeysStore(contract.code(contract.keyProviders, provider.wireName), buffer, bytes.length),
    );
  } finally {
    buffer.asTypedList(bytes.isEmpty ? 1 : bytes.length).fillRange(0, bytes.isEmpty ? 1 : bytes.length, 0);
    bytes.fillRange(0, bytes.length, 0);
    calloc.free(buffer);
  }
}

/// Remove [provider]'s key from the system's secure storage.
void deleteKinetoKey(KinetoKeyProvider provider) {
  _assertAbiCompatible();
  final contract = KinetoNativeContract.instance;
  _checkProjectResult(_kinetoKeysDelete(contract.code(contract.keyProviders, provider.wireName)));
}

/// [loadKinetoKeys] on another isolate.
Future<KinetoKeys> loadKinetoKeysInBackground() => Isolate.run(loadKinetoKeys);

/// [storeKinetoKey] on another isolate.
Future<void> storeKinetoKeyInBackground(KinetoKeyProvider provider, String key) =>
    Isolate.run(() => storeKinetoKey(provider, key));

/// [deleteKinetoKey] on another isolate.
Future<void> deleteKinetoKeyInBackground(KinetoKeyProvider provider) =>
    Isolate.run(() => deleteKinetoKey(provider));
