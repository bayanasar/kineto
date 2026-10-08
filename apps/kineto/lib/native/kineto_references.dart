part of 'kineto_engine.dart';

/// One argument of a tool call, laid out as `KinetoToolArgument`.
final class _NativeToolArgument extends Struct {
  external _NativeText name;

  @Uint32()
  external int kind;

  external _NativeText value;
}

@Native<
    Int32 Function(
      Pointer<Void>,
      Pointer<Uint8>,
      Uint64,
      Pointer<Uint8>,
      Uint64,
      Pointer<_NativeToolArgument>,
      Uint64,
      Pointer<_NativeText>,
      Uint64,
      Uint64,
    )>(symbol: 'kineto_project_ask_reference')
external int _kinetoProjectAskReference(
  Pointer<Void> session,
  Pointer<Uint8> server,
  int serverLength,
  Pointer<Uint8> tool,
  int toolLength,
  Pointer<_NativeToolArgument> arguments,
  int argumentCount,
  Pointer<_NativeText> targets,
  int targetCount,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Pointer<Uint8>, Uint64, Pointer<Uint8>, Uint64, Uint64)>(
  symbol: 'kineto_project_edit_reference',
)
external int _kinetoProjectEditReference(
  Pointer<Void> session,
  Pointer<Uint8> id,
  int idLength,
  Pointer<Uint8> text,
  int textLength,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Pointer<Uint8>, Uint64, Pointer<Uint8>, Uint64, Bool, Uint64)>(
  symbol: 'kineto_project_attach_reference',
)
external int _kinetoProjectAttachReference(
  Pointer<Void> session,
  Pointer<Uint8> id,
  int idLength,
  Pointer<Uint8> target,
  int targetLength,
  bool attach,
  int token,
);

@Native<Int32 Function(Pointer<Void>, Pointer<Uint8>, Uint64, Uint64)>(symbol: 'kineto_project_remove_reference')
external int _kinetoProjectRemoveReference(Pointer<Void> session, Pointer<Uint8> id, int idLength, int token);

@Native<Uint32 Function(Pointer<Void>)>(symbol: 'kineto_project_reference_count', isLeaf: true)
external int _kinetoProjectReferenceCount(Pointer<Void> session);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Pointer<Uint8>, Uint64, Pointer<Uint64>)>(
  symbol: 'kineto_project_reference_text_copy',
  isLeaf: true,
)
external int _kinetoProjectReferenceTextCopy(
  Pointer<Void> session,
  int index,
  int field,
  Pointer<Uint8> buffer,
  int capacity,
  Pointer<Uint64> length,
);

@Native<Int32 Function(Pointer<Pointer<Void>>)>(symbol: 'kineto_mcp_load')
external int _kinetoMcpLoad(Pointer<Pointer<Void>> outServers);

@Native<Uint32 Function(Pointer<Void>, Uint32, Uint32)>(symbol: 'kineto_mcp_count', isLeaf: true)
external int _kinetoMcpCount(Pointer<Void> servers, int server, int tool);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Uint32, Uint32, Pointer<Uint64>)>(
  symbol: 'kineto_mcp_value',
  isLeaf: true,
)
external int _kinetoMcpValue(Pointer<Void> servers, int server, int tool, int parameter, int field, Pointer<Uint64> out);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Uint32, Uint32, Pointer<Uint8>, Uint64, Pointer<Uint64>)>(
  symbol: 'kineto_mcp_text_copy',
  isLeaf: true,
)
external int _kinetoMcpTextCopy(
  Pointer<Void> servers,
  int server,
  int tool,
  int parameter,
  int field,
  Pointer<Uint8> buffer,
  int capacity,
  Pointer<Uint64> length,
);

@Native<Void Function(Pointer<Void>)>(symbol: 'kineto_mcp_destroy')
external void _kinetoMcpDestroy(Pointer<Void> servers);

/// No server, tool or parameter: the level above is meant.
const int _none = 0xFFFFFFFF;

/// What a reference can be about: the world.
const String kinetoReferenceWorld = 'bible_world';

/// The target a reference about bible [entry] names.
String kinetoReferenceTarget(KinetoBibleEntry entry) => 'bible_${entry.kind.wireName}.${entry.id}';

/// [target] in words: `the world`, or the entry's kind and ID.
String describeReferenceTarget(String target) {
  if (target == kinetoReferenceWorld) return 'the world';
  final rest = target.startsWith('bible_') ? target.substring(6) : target;
  final dot = rest.indexOf('.');
  return dot < 0 ? rest : '${rest.substring(0, dot)} ${rest.substring(dot + 1)}';
}

/// How a tool takes an argument.
enum KinetoToolArgumentKind {
  text('text'),
  number('number'),
  flag('flag');

  const KinetoToolArgumentKind(this.wireName);

  final String wireName;
}

/// One argument the director gives a tool.
final class KinetoToolArgument {
  const KinetoToolArgument(this.name, this.value, {this.kind = KinetoToolArgumentKind.text});

  final String name;
  final KinetoToolArgumentKind kind;

  /// A number in decimal, a flag as `true` or `false`.
  final String value;
}

/// Text the director brought in from an MCP tool, as material for the
/// bible (ADR-0007).
final class KinetoReference {
  const KinetoReference({
    required this.id,
    required this.server,
    required this.tool,
    required this.text,
    this.arguments = const <String>[],
    this.attachedTo = const <String>[],
    this.contentHash = '',
  });

  final String id;
  final String server;
  final String tool;

  /// What the tool was asked with, as `name: value`.
  final List<String> arguments;
  final String text;

  /// What it is about: [kinetoReferenceWorld] or bible entry targets.
  final List<String> attachedTo;
  final String contentHash;
}

/// One argument a tool takes.
final class KinetoMcpParameter {
  const KinetoMcpParameter({
    required this.name,
    this.description = '',
    this.kind,
    this.required = false,
  });

  final String name;
  final String description;

  /// Null for an argument the director cannot give in a field.
  final KinetoToolArgumentKind? kind;
  final bool required;
}

/// A tool a server offers and its entry allows.
final class KinetoMcpTool {
  const KinetoMcpTool({required this.name, this.description = '', this.parameters = const <KinetoMcpParameter>[]});

  final String name;
  final String description;
  final List<KinetoMcpParameter> parameters;
}

/// An MCP server the user registered in `mcp.toml`. Read only: servers are
/// registered in the configuration file, never through the app.
final class KinetoMcpServer {
  const KinetoMcpServer({
    required this.id,
    required this.executable,
    this.secrets = const <String>[],
    this.allowed = const <String>[],
    this.enabled = true,
    this.tools,
    this.problem = '',
  });

  final String id;
  final String executable;
  final List<String> secrets;

  /// The tools the entry allows, by name.
  final List<String> allowed;
  final bool enabled;

  /// The allowed tools it offers; null when it is disabled or could not be
  /// asked.
  final List<KinetoMcpTool>? tools;

  /// Why its tools could not be listed; empty otherwise.
  final String problem;
}

/// The MCP servers the user registered, each enabled one started once to
/// list its tools. That can take as long as a slow server's deadline, so
/// call it off the UI thread, as [loadKinetoMcpServersInBackground] does.
List<KinetoMcpServer> loadKinetoMcpServers() {
  _assertAbiCompatible();
  final contract = KinetoNativeContract.instance;
  final arena = Arena();
  try {
    final out = arena<Pointer<Void>>();
    _checkProjectResult(_kinetoMcpLoad(out));
    final handle = out.value;
    try {
      final value = arena<Uint64>();
      int number(int server, String field, {int tool = _none, int parameter = _none}) {
        _checkProjectResult(
          _kinetoMcpValue(handle, server, tool, parameter, contract.code(contract.mcpValues, field), value),
        );
        return value.value;
      }

      String text(int server, String field, {int tool = _none, int parameter = _none}) {
        final code = contract.code(contract.mcpTexts, field);
        return _copyProjectUtf8(
          handle,
          (servers, buffer, capacity, length) =>
              _kinetoMcpTextCopy(servers, server, tool, parameter, code, buffer, capacity, length),
          field,
        );
      }

      List<String> lines(int server, String field) =>
          text(server, field).split('\n').where((line) => line.isNotEmpty).toList(growable: false);

      KinetoToolArgumentKind? kind(int code) {
        for (final kind in KinetoToolArgumentKind.values) {
          if (contract.code(contract.toolArgumentKinds, kind.wireName) == code) return kind;
        }
        return null;
      }

      return <KinetoMcpServer>[
        for (var server = 0; server < _kinetoMcpCount(handle, _none, _none); server++)
          KinetoMcpServer(
            id: text(server, 'id'),
            executable: text(server, 'executable'),
            secrets: lines(server, 'secrets'),
            allowed: lines(server, 'allowed'),
            enabled: number(server, 'enabled') != 0,
            problem: text(server, 'problem'),
            tools: number(server, 'listed') == 0
                ? null
                : <KinetoMcpTool>[
                    for (var tool = 0; tool < _kinetoMcpCount(handle, server, _none); tool++)
                      KinetoMcpTool(
                        name: text(server, 'tool_name', tool: tool),
                        description: text(server, 'tool_description', tool: tool),
                        parameters: <KinetoMcpParameter>[
                          for (var parameter = 0; parameter < _kinetoMcpCount(handle, server, tool); parameter++)
                            KinetoMcpParameter(
                              name: text(server, 'parameter_name', tool: tool, parameter: parameter),
                              description: text(server, 'parameter_description', tool: tool, parameter: parameter),
                              kind: kind(number(server, 'kind', tool: tool, parameter: parameter)),
                              required: number(server, 'required', tool: tool, parameter: parameter) != 0,
                            ),
                        ],
                      ),
                  ],
          ),
      ];
    } finally {
      _kinetoMcpDestroy(handle);
    }
  } finally {
    arena.releaseAll();
  }
}

/// [loadKinetoMcpServers] on another isolate.
Future<List<KinetoMcpServer>> loadKinetoMcpServersInBackground() => Isolate.run(loadKinetoMcpServers);

extension KinetoProjectReferences on KinetoProjectSession {
  /// The references the director brought in, oldest first, as the worker
  /// last published them.
  List<KinetoReference> get references {
    _ensureOpen();
    final contract = KinetoNativeContract.instance;
    String text(int index, String field) {
      final code = contract.code(contract.referenceTexts, field);
      return _copyProjectUtf8(
        _handle,
        (session, buffer, capacity, length) =>
            _kinetoProjectReferenceTextCopy(session, index, code, buffer, capacity, length),
        field,
      );
    }

    List<String> lines(int index, String field) =>
        text(index, field).split('\n').where((line) => line.isNotEmpty).toList(growable: false);

    return <KinetoReference>[
      for (var index = 0; index < _kinetoProjectReferenceCount(_handle); index++)
        KinetoReference(
          id: text(index, 'id'),
          server: text(index, 'server'),
          tool: text(index, 'tool'),
          arguments: lines(index, 'arguments'),
          text: text(index, 'text'),
          attachedTo: lines(index, 'attached_to'),
          contentHash: text(index, 'content_hash'),
        ),
    ];
  }

  /// Call [tool] of the registered MCP server [server] with [arguments],
  /// and keep its text as a reference about [attachedTo]. Only the
  /// director calls a tool; nothing it answers is acted on.
  Future<void> askReference({
    required String server,
    required String tool,
    List<KinetoToolArgument> arguments = const <KinetoToolArgument>[],
    List<String> attachedTo = const <String>[],
  }) {
    final contract = KinetoNativeContract.instance;
    return _submitDocument((token) {
      final arena = Arena();
      try {
        final serverText = _encodeUtf8(arena, server);
        final toolText = _encodeUtf8(arena, tool);
        final nativeArguments = arena<_NativeToolArgument>(arguments.isEmpty ? 1 : arguments.length);
        for (var index = 0; index < arguments.length; index++) {
          final argument = arguments[index];
          final name = _encodeUtf8(arena, argument.name);
          final value = _encodeUtf8(arena, argument.value);
          nativeArguments[index]
            ..name.pointer = name.pointer
            ..name.length = name.length
            ..kind = contract.code(contract.toolArgumentKinds, argument.kind.wireName)
            ..value.pointer = value.pointer
            ..value.length = value.length;
        }
        final targets = arena<_NativeText>(attachedTo.isEmpty ? 1 : attachedTo.length);
        for (var index = 0; index < attachedTo.length; index++) {
          final target = _encodeUtf8(arena, attachedTo[index]);
          targets[index]
            ..pointer = target.pointer
            ..length = target.length;
        }
        return _kinetoProjectAskReference(
          _handle,
          serverText.pointer,
          serverText.length,
          toolText.pointer,
          toolText.length,
          nativeArguments,
          arguments.length,
          targets,
          attachedTo.length,
          token,
        );
      } finally {
        arena.releaseAll();
      }
    });
  }

  /// Replace reference [id]'s text with the director's [text].
  Future<void> editReference(String id, String text) => _submitDocument((token) {
        final arena = Arena();
        try {
          final reference = _encodeUtf8(arena, id);
          final words = _encodeUtf8(arena, text);
          return _kinetoProjectEditReference(
            _handle,
            reference.pointer,
            reference.length,
            words.pointer,
            words.length,
            token,
          );
        } finally {
          arena.releaseAll();
        }
      });

  /// Attach reference [id] to [target], or detach it when not [attach].
  Future<void> attachReference(String id, String target, {bool attach = true}) => _submitDocument((token) {
        final arena = Arena();
        try {
          final reference = _encodeUtf8(arena, id);
          final about = _encodeUtf8(arena, target);
          return _kinetoProjectAttachReference(
            _handle,
            reference.pointer,
            reference.length,
            about.pointer,
            about.length,
            attach,
            token,
          );
        } finally {
          arena.releaseAll();
        }
      });

  Future<void> removeReference(String id) => _submitDocument((token) {
        final arena = Arena();
        try {
          final reference = _encodeUtf8(arena, id);
          return _kinetoProjectRemoveReference(_handle, reference.pointer, reference.length, token);
        } finally {
          arena.releaseAll();
        }
      });
}
