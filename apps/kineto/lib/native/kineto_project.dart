part of 'kineto_engine.dart';

typedef _DestroyProjectNative = Void Function(Pointer<Void>);
typedef _ProjectTextCopyDart = int Function(
  Pointer<Void>,
  Pointer<Uint8>,
  int,
  Pointer<Uint64>,
);

@Native<
    Int32 Function(
      Pointer<Uint8>,
      Uint64,
      Pointer<Uint8>,
      Uint64,
      Pointer<Uint8>,
      Uint64,
      Pointer<Uint8>,
      Uint64,
      Pointer<Uint8>,
      Uint64,
      Pointer<Uint8>,
      Uint64,
      Pointer<Pointer<Void>>,
    )>(symbol: 'kineto_project_create_text')
external int _kinetoProjectCreateText(
  Pointer<Uint8> path,
  int pathLength,
  Pointer<Uint8> projectId,
  int projectIdLength,
  Pointer<Uint8> title,
  int titleLength,
  Pointer<Uint8> createdAt,
  int createdAtLength,
  Pointer<Uint8> language,
  int languageLength,
  Pointer<Uint8> source,
  int sourceLength,
  Pointer<Pointer<Void>> outSession,
);

@Native<
    Int32 Function(
      Pointer<Uint8>,
      Uint64,
      Pointer<Pointer<Void>>,
    )>(symbol: 'kineto_project_open')
external int _kinetoProjectOpen(
  Pointer<Uint8> path,
  int pathLength,
  Pointer<Pointer<Void>> outSession,
);

@Native<_DestroyProjectNative>(symbol: 'kineto_project_destroy')
external void _kinetoProjectDestroy(Pointer<Void> session);

@Native<Uint32 Function(Pointer<Void>)>(
  symbol: 'kineto_project_format_version',
  isLeaf: true,
)
external int _kinetoProjectFormatVersion(Pointer<Void> session);

@Native<Uint8 Function(Pointer<Void>)>(
  symbol: 'kineto_project_is_read_only',
  isLeaf: true,
)
external int _kinetoProjectIsReadOnly(Pointer<Void> session);

@Native<Int32 Function(Pointer<Void>, Pointer<Uint8>, Uint64, Pointer<Uint64>)>(
  symbol: 'kineto_project_id_copy',
)
external int _kinetoProjectIdCopy(
  Pointer<Void> session,
  Pointer<Uint8> outBuffer,
  int outCapacity,
  Pointer<Uint64> outLength,
);

@Native<Int32 Function(Pointer<Void>, Pointer<Uint8>, Uint64, Pointer<Uint64>)>(
  symbol: 'kineto_project_title_copy',
)
external int _kinetoProjectTitleCopy(
  Pointer<Void> session,
  Pointer<Uint8> outBuffer,
  int outCapacity,
  Pointer<Uint64> outLength,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Pointer<Uint64>)>(
  symbol: 'kineto_project_shot_state',
)
external int _kinetoProjectShotState(
  Pointer<Void> session,
  int shotIndex,
  Pointer<Uint64> outState,
);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32)>(
  symbol: 'kineto_project_shot_set_direction',
)
external int _kinetoProjectShotSetDirection(
  Pointer<Void> session,
  int shotIndex,
  int direction,
);

@Native<Int32 Function(Pointer<Void>, Uint32)>(
  symbol: 'kineto_project_shot_generate',
)
external int _kinetoProjectShotGenerate(Pointer<Void> session, int shotIndex);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32)>(
  symbol: 'kineto_project_shot_select',
)
external int _kinetoProjectShotSelect(
  Pointer<Void> session,
  int shotIndex,
  int candidateIndex,
);

@Native<Int32 Function(Pointer<Void>, Uint32)>(
  symbol: 'kineto_project_shot_lock',
)
external int _kinetoProjectShotLock(Pointer<Void> session, int shotIndex);

@Native<Int32 Function(Pointer<Void>, Uint32)>(
  symbol: 'kineto_project_shot_reset',
)
external int _kinetoProjectShotReset(Pointer<Void> session, int shotIndex);

final class KinetoProjectException implements Exception {
  const KinetoProjectException(this.code, this.message);

  final int code;
  final String message;

  @override
  String toString() => 'KinetoProjectException($code): $message';
}

/// Thin Dart owner for an opened canonical Kineto project.
///
/// Strings cross the native boundary only as borrowed UTF-8 pointer/length
/// pairs. Metadata and shot snapshots come back in caller-owned fixed-width
/// storage; TOML, JSON, and serialized project models do not cross FFI.
final class KinetoProjectSession implements Finalizable {
  KinetoProjectSession._(this._handle) {
    _finalizer.attach(this, _handle, detach: this);
  }

  static const int shotCount = 2;

  static final NativeFinalizer _finalizer = NativeFinalizer(
    Native.addressOf<NativeFunction<_DestroyProjectNative>>(
      _kinetoProjectDestroy,
    ),
  );

  final Pointer<Void> _handle;
  bool _closed = false;

  factory KinetoProjectSession.createTextProject({
    required String path,
    required String projectId,
    required String title,
    required String createdAt,
    required String language,
    required String sourceText,
  }) {
    _assertAbiCompatible();
    final arena = Arena();
    try {
      final pathBytes = _encodeUtf8(arena, path);
      final idBytes = _encodeUtf8(arena, projectId);
      final titleBytes = _encodeUtf8(arena, title);
      final createdAtBytes = _encodeUtf8(arena, createdAt);
      final languageBytes = _encodeUtf8(arena, language);
      final sourceBytes = _encodeUtf8(arena, sourceText);
      final out = arena<Pointer<Void>>();
      out.value = nullptr;

      final code = _kinetoProjectCreateText(
        pathBytes.pointer,
        pathBytes.length,
        idBytes.pointer,
        idBytes.length,
        titleBytes.pointer,
        titleBytes.length,
        createdAtBytes.pointer,
        createdAtBytes.length,
        languageBytes.pointer,
        languageBytes.length,
        sourceBytes.pointer,
        sourceBytes.length,
        out,
      );
      _checkProjectResult(code);
      if (out.value == nullptr) {
        throw StateError('Kineto native project create returned a null session');
      }
      return KinetoProjectSession._(out.value);
    } finally {
      arena.releaseAll();
    }
  }

  factory KinetoProjectSession.open(String path) {
    _assertAbiCompatible();
    final arena = Arena();
    try {
      final pathBytes = _encodeUtf8(arena, path);
      final out = arena<Pointer<Void>>();
      out.value = nullptr;
      final code = _kinetoProjectOpen(
        pathBytes.pointer,
        pathBytes.length,
        out,
      );
      _checkProjectResult(code);
      if (out.value == nullptr) {
        throw StateError('Kineto native project open returned a null session');
      }
      return KinetoProjectSession._(out.value);
    } finally {
      arena.releaseAll();
    }
  }

  int get formatVersion {
    _ensureOpen();
    return _kinetoProjectFormatVersion(_handle);
  }

  bool get isReadOnly {
    _ensureOpen();
    return _kinetoProjectIsReadOnly(_handle) != 0;
  }

  String get projectId {
    _ensureOpen();
    return _copyProjectUtf8(_handle, _kinetoProjectIdCopy, 'project id');
  }

  String get title {
    _ensureOpen();
    return _copyProjectUtf8(_handle, _kinetoProjectTitleCopy, 'project title');
  }

  KinetoShotSnapshot shotSnapshot(int shotIndex) {
    _ensureOpen();
    _checkShotIndex(shotIndex);
    final arena = Arena();
    try {
      final outState = arena<Uint64>();
      outState.value = 0;
      _checkProjectResult(
        _kinetoProjectShotState(_handle, shotIndex, outState),
      );
      return KinetoShotSnapshot.fromBits(outState.value);
    } finally {
      arena.releaseAll();
    }
  }

  void setShotDirection(int shotIndex, KinetoShotDirection direction) {
    _ensureOpen();
    _checkShotIndex(shotIndex);
    _checkProjectResult(
      _kinetoProjectShotSetDirection(_handle, shotIndex, direction.wireValue),
    );
  }

  void generateShotCandidates(int shotIndex) {
    _ensureOpen();
    _checkShotIndex(shotIndex);
    _checkProjectResult(_kinetoProjectShotGenerate(_handle, shotIndex));
  }

  void selectShotCandidate(int shotIndex, int candidateIndex) {
    _ensureOpen();
    _checkShotIndex(shotIndex);
    if (candidateIndex < 0) {
      throw RangeError.value(
        candidateIndex,
        'candidateIndex',
        'Candidate index must be non-negative',
      );
    }
    _checkProjectResult(
      _kinetoProjectShotSelect(_handle, shotIndex, candidateIndex),
    );
  }

  void lockShotSelection(int shotIndex) {
    _ensureOpen();
    _checkShotIndex(shotIndex);
    _checkProjectResult(_kinetoProjectShotLock(_handle, shotIndex));
  }

  void resetShot(int shotIndex) {
    _ensureOpen();
    _checkShotIndex(shotIndex);
    _checkProjectResult(_kinetoProjectShotReset(_handle, shotIndex));
  }

  void close() {
    if (_closed) return;
    _closed = true;
    _finalizer.detach(this);
    _kinetoProjectDestroy(_handle);
  }

  void _checkShotIndex(int shotIndex) {
    if (shotIndex < 0 || shotIndex >= shotCount) {
      throw RangeError.range(shotIndex, 0, shotCount - 1, 'shotIndex');
    }
  }

  void _ensureOpen() {
    if (_closed) {
      throw StateError('Kineto project session is closed');
    }
  }
}

final class _NativeBytes {
  const _NativeBytes(this.pointer, this.length);

  final Pointer<Uint8> pointer;
  final int length;
}

_NativeBytes _encodeUtf8(Arena arena, String value) {
  final bytes = utf8.encode(value);
  final pointer = arena<Uint8>(bytes.isEmpty ? 1 : bytes.length);
  if (bytes.isNotEmpty) {
    pointer.asTypedList(bytes.length).setAll(0, bytes);
  }
  return _NativeBytes(pointer, bytes.length);
}

String _copyProjectUtf8(
  Pointer<Void> session,
  _ProjectTextCopyDart copy,
  String field,
) {
  final arena = Arena();
  try {
    final outLength = arena<Uint64>();
    outLength.value = 0;
    final sizingCode = copy(session, nullptr, 0, outLength);
    if (sizingCode == 0 && outLength.value == 0) return '';
    if (sizingCode != 106) _checkProjectResult(sizingCode);

    final capacity = outLength.value;
    if (capacity == 0) return '';
    final buffer = arena<Uint8>(capacity);
    outLength.value = 0;
    final copyCode = copy(session, buffer, capacity, outLength);
    if (copyCode == 106) {
      throw StateError('Kineto native $field changed while it was being copied');
    }
    _checkProjectResult(copyCode);
    if (outLength.value > capacity) {
      throw StateError('Kineto native $field length exceeded the allocated buffer');
    }
    return utf8.decode(
      buffer.asTypedList(outLength.value),
      allowMalformed: false,
    );
  } finally {
    arena.releaseAll();
  }
}

void _checkProjectResult(int code) {
  if (code == 0) return;
  throw KinetoProjectException(code, switch (code) {
    100 => 'Invalid project argument',
    101 => 'Project input is not valid UTF-8',
    102 => 'Project filesystem operation failed',
    103 => 'Project target already exists',
    104 => 'Project manifest is invalid',
    105 => 'Project format is newer than this Kineto build supports',
    106 => 'Project metadata buffer is too small',
    107 => 'Project is read-only until an explicit migration is performed',
    108 => 'Shot index is invalid',
    109 => 'Generate candidates before selecting',
    110 => 'Candidate index is invalid',
    111 => 'Shot selection is locked',
    112 => 'Shot selection is already locked',
    113 => 'Select a candidate before locking',
    114 => 'Selection is stale; regenerate before locking',
    115 => 'Canonical shot approval state is invalid',
    116 => 'Shot generation revision overflow',
    199 => 'Native project call failed unexpectedly',
    _ => 'Unknown native project error',
  });
}
