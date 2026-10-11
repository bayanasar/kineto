part of 'kineto_engine.dart';

@Native<Int32 Function()>(symbol: 'kineto_mcp_server_resume')
external int _kinetoMcpServerResume();

@Native<Int32 Function(Uint8)>(symbol: 'kineto_mcp_server_set_enabled')
external int _kinetoMcpServerSetEnabled(int enabled);

@Native<Int32 Function()>(symbol: 'kineto_mcp_server_renew')
external int _kinetoMcpServerRenew();

@Native<Int32 Function(Pointer<Uint8>, Uint64, Pointer<Uint64>)>(symbol: 'kineto_mcp_server_address_copy')
external int _kinetoMcpServerAddressCopy(Pointer<Uint8> buffer, int capacity, Pointer<Uint64> length);

/// Whether agents outside Kineto can read the open project over MCP, and
/// at which address (ADR-0010).
final class KinetoServing {
  const KinetoServing({this.address = ''});

  /// Where an agent connects while Kineto serves; empty while it does not.
  final String address;

  bool get on => address.isNotEmpty;

  /// How Claude Code adds it.
  String get claudeCode => 'claude mcp add --transport http kineto $address';

  /// How Codex adds it, in `~/.codex/config.toml`.
  String get codex => '[mcp_servers.kineto]\nurl = "$address"';
}

KinetoServing _serving() => KinetoServing(
      address: _copyProjectUtf8(
        nullptr,
        (_, buffer, capacity, length) => _kinetoMcpServerAddressCopy(buffer, capacity, length),
        'MCP server address',
      ),
    );

/// Serve outside agents if the director left it on and Kineto does not
/// yet, as it starts and when Settings read whether it does: a reason it
/// cannot is thrown. Reads a file and binds a port, so call it off the UI
/// thread, as [resumeKinetoServingInBackground] does.
KinetoServing resumeKinetoServing() {
  _assertAbiCompatible();
  _checkProjectResult(_kinetoMcpServerResume());
  return _serving();
}

/// Start or stop serving outside agents; the address stays the same.
KinetoServing setKinetoServing(bool on) {
  _assertAbiCompatible();
  _checkProjectResult(_kinetoMcpServerSetEnabled(on ? 1 : 0));
  return _serving();
}

/// A new address: the old one stops working at once.
KinetoServing renewKinetoServingAddress() {
  _assertAbiCompatible();
  _checkProjectResult(_kinetoMcpServerRenew());
  return _serving();
}

Future<KinetoServing> resumeKinetoServingInBackground() => Isolate.run(resumeKinetoServing);

Future<KinetoServing> setKinetoServingInBackground(bool on) => Isolate.run(() => setKinetoServing(on));

Future<KinetoServing> renewKinetoServingAddressInBackground() => Isolate.run(renewKinetoServingAddress);

@Native<Int32 Function(Uint8, Pointer<Uint8>, Uint64, Pointer<Uint8>, Uint64)>(symbol: 'kineto_mcp_server_set_public')
external int _kinetoMcpServerSetPublic(
    int public, Pointer<Uint8> url, int urlLength, Pointer<Uint8> tunnel, int tunnelLength);

@Native<Int32 Function(Pointer<Pointer<Void>>)>(symbol: 'kineto_mcp_server_public_load')
external int _kinetoMcpServerPublicLoad(Pointer<Pointer<Void>> out);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Pointer<Uint64>)>(
  symbol: 'kineto_mcp_server_public_value',
  isLeaf: true,
)
external int _kinetoMcpServerPublicValue(Pointer<Void> public, int index, int field, Pointer<Uint64> out);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Pointer<Uint8>, Uint64, Pointer<Uint64>)>(
  symbol: 'kineto_mcp_server_public_text_copy',
  isLeaf: true,
)
external int _kinetoMcpServerPublicTextCopy(
    Pointer<Void> public, int index, int field, Pointer<Uint8> buffer, int capacity, Pointer<Uint64> length);

@Native<Void Function(Pointer<Void>)>(symbol: 'kineto_mcp_server_public_destroy')
external void _kinetoMcpServerPublicDestroy(Pointer<Void> public);

@Native<Int32 Function(Pointer<Uint8>, Uint64)>(symbol: 'kineto_mcp_server_disconnect')
external int _kinetoMcpServerDisconnect(Pointer<Uint8> id, int idLength);

@Native<Int32 Function(Pointer<Pointer<Void>>)>(symbol: 'kineto_mcp_server_asks_load')
external int _kinetoMcpServerAsksLoad(Pointer<Pointer<Void>> out);

@Native<Uint32 Function(Pointer<Void>)>(symbol: 'kineto_mcp_server_asks_count', isLeaf: true)
external int _kinetoMcpServerAsksCount(Pointer<Void> asks);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Pointer<Uint64>)>(
  symbol: 'kineto_mcp_server_asks_value',
  isLeaf: true,
)
external int _kinetoMcpServerAsksValue(Pointer<Void> asks, int index, int field, Pointer<Uint64> out);

@Native<Int32 Function(Pointer<Void>, Uint32, Uint32, Pointer<Uint8>, Uint64, Pointer<Uint64>)>(
  symbol: 'kineto_mcp_server_asks_text_copy',
  isLeaf: true,
)
external int _kinetoMcpServerAsksTextCopy(
    Pointer<Void> asks, int index, int field, Pointer<Uint8> buffer, int capacity, Pointer<Uint64> length);

@Native<Void Function(Pointer<Void>)>(symbol: 'kineto_mcp_server_asks_destroy')
external void _kinetoMcpServerAsksDestroy(Pointer<Void> asks);

/// The number and text fields of what waits for the director, and of where
/// agents in the cloud stand, that these bindings read.
const Set<String> kinetoAgentAskValueFields = <String>{'id', 'connect'};
const Set<String> kinetoAgentAskTextFields = <String>{'agent', 'what', 'cost'};
const Set<String> kinetoPublicValueFields = <String>{
  'public',
  'listening',
  'tunnel_running',
  'port',
  'agents',
  'agent_since',
};
const Set<String> kinetoPublicTextFields = <String>{
  'url',
  'address',
  'tunnel',
  'tunnel_said',
  'problem',
  'agent_id',
  'agent_name',
  'agent_site',
};

@Native<Int32 Function(Uint64, Uint8)>(symbol: 'kineto_mcp_server_decide')
external int _kinetoMcpServerDecide(int id, int allow);

/// An agent the director allowed to connect from the internet.
final class KinetoConnectedAgent {
  const KinetoConnectedAgent({required this.id, required this.name, required this.site, required this.since});


  final String id;
  final String name;

  /// Where it sends the director back to, such as chatgpt.com.
  final String site;
  final DateTime since;
}

/// Whether agents in the cloud, such as ChatGPT and claude.ai, can connect
/// through the director's tunnel, and who did (ADR-0011).
final class KinetoPublicServing {
  const KinetoPublicServing({
    this.public = false,
    this.url = '',
    this.address = '',
    this.tunnel = const <String>[],
    this.port = 0,
    this.listening = false,
    this.tunnelRunning = false,
    this.tunnelSaid = '',
    this.problem,
    this.agents = const <KinetoConnectedAgent>[],
  });


  /// Whether the director let agents in the cloud connect.
  final bool public;

  /// The HTTPS origin the tunnel answers at.
  final String url;

  /// What an agent in the cloud connects to; empty while Kineto does not
  /// listen for them.
  final String address;

  /// The command that opens the tunnel, word by word.
  final List<String> tunnel;

  /// The port on this computer the tunnel forwards to.
  final int port;
  final bool listening;
  final bool tunnelRunning;

  /// What the tunnel last said on its error stream.
  final String tunnelSaid;

  /// Why the tunnel did not start.
  final String? problem;
  final List<KinetoConnectedAgent> agents;
}

/// Something an agent asked that waits for the director in Kineto's window.
final class KinetoAgentAsk {
  const KinetoAgentAsk({required this.id, required this.connect, required this.agent, required this.what, this.cost});


  final int id;

  /// An agent asking to connect from the internet, rather than a command.
  final bool connect;
  final String agent;

  /// What it asks, such as “make keyframes of s01/sh02”.
  final String what;

  /// What a paid command costs, as far as Kineto knows.
  final String? cost;

  @override
  bool operator ==(Object other) => other is KinetoAgentAsk && other.id == id;

  @override
  int get hashCode => id.hashCode;
}

KinetoPublicServing _publicServing() {
  final contract = KinetoNativeContract.instance;
  final arena = Arena();
  try {
    final out = arena<Pointer<Void>>();
    _checkProjectResult(_kinetoMcpServerPublicLoad(out));
    final handle = out.value;
    try {
      final value = arena<Uint64>();
      int number(String field, [int index = 0]) {
        _checkProjectResult(
            _kinetoMcpServerPublicValue(handle, index, contract.code(contract.publicValues, field), value));
        return value.value;
      }

      String text(String field, [int index = 0]) {
        final code = contract.code(contract.publicTexts, field);
        return _copyProjectUtf8(
          handle,
          (public, buffer, capacity, length) =>
              _kinetoMcpServerPublicTextCopy(public, index, code, buffer, capacity, length),
          field,
        );
      }

      final problem = text('problem');
      return KinetoPublicServing(
        public: number('public') != 0,
        url: text('url'),
        address: text('address'),
        tunnel: text('tunnel').split('\n').where((word) => word.isNotEmpty).toList(growable: false),
        port: number('port'),
        listening: number('listening') != 0,
        tunnelRunning: number('tunnel_running') != 0,
        tunnelSaid: text('tunnel_said'),
        problem: problem.isEmpty ? null : problem,
        agents: <KinetoConnectedAgent>[
          for (var index = 0; index < number('agents'); index++)
            KinetoConnectedAgent(
              id: text('agent_id', index),
              name: text('agent_name', index),
              site: text('agent_site', index),
              since: DateTime.fromMillisecondsSinceEpoch(number('agent_since', index) * 1000, isUtc: true),
            ),
        ],
      );
    } finally {
      _kinetoMcpServerPublicDestroy(handle);
    }
  } finally {
    arena.releaseAll();
  }
}

/// Where agents in the cloud stand. Reads files, so call it off the UI
/// thread, as [readKinetoPublicServingInBackground] does.
KinetoPublicServing readKinetoPublicServing() {
  _assertAbiCompatible();
  return _publicServing();
}

/// Let agents in the cloud connect at [url], an HTTPS origin, through the
/// tunnel [tunnel] opens (`{port}` for Kineto's port), or not. Kineto
/// listens for them at once when it serves.
KinetoPublicServing setKinetoPublicServing({required bool on, required String url, required List<String> tunnel}) {
  _assertAbiCompatible();
  _withUtf8(url.trim(), (urlBytes, urlLength) {
    _withUtf8(tunnel.map((word) => word.trim()).where((word) => word.isNotEmpty).join('\n'),
        (tunnelBytes, tunnelLength) {
      _checkProjectResult(_kinetoMcpServerSetPublic(on ? 1 : 0, urlBytes, urlLength, tunnelBytes, tunnelLength));
    });
  });
  return _publicServing();
}

/// Forget an agent the director allowed: it must be allowed again.
KinetoPublicServing disconnectKinetoAgent(String id) {
  _assertAbiCompatible();
  _withUtf8(id, (bytes, length) => _checkProjectResult(_kinetoMcpServerDisconnect(bytes, length)));
  return _publicServing();
}

Future<KinetoPublicServing> readKinetoPublicServingInBackground() => Isolate.run(readKinetoPublicServing);

Future<KinetoPublicServing> setKinetoPublicServingInBackground({
  required bool on,
  required String url,
  required List<String> tunnel,
}) =>
    Isolate.run(() => setKinetoPublicServing(on: on, url: url, tunnel: tunnel));

Future<KinetoPublicServing> disconnectKinetoAgentInBackground(String id) =>
    Isolate.run(() => disconnectKinetoAgent(id));

/// What agents asked that waits for the director, oldest first. Touches
/// only memory, so the UI thread may call it.
List<KinetoAgentAsk> kinetoAgentAsks() {
  _assertAbiCompatible();
  final contract = KinetoNativeContract.instance;
  final arena = Arena();
  try {
    final out = arena<Pointer<Void>>();
    _checkProjectResult(_kinetoMcpServerAsksLoad(out));
    final handle = out.value;
    try {
      final value = arena<Uint64>();
      int number(int index, String field) {
        _checkProjectResult(
            _kinetoMcpServerAsksValue(handle, index, contract.code(contract.agentAskValues, field), value));
        return value.value;
      }

      String text(int index, String field) {
        final code = contract.code(contract.agentAskTexts, field);
        return _copyProjectUtf8(
          handle,
          (asks, buffer, capacity, length) => _kinetoMcpServerAsksTextCopy(asks, index, code, buffer, capacity, length),
          field,
        );
      }

      return <KinetoAgentAsk>[
        for (var index = 0; index < _kinetoMcpServerAsksCount(handle); index++)
          KinetoAgentAsk(
            id: number(index, 'id'),
            connect: number(index, 'connect') != 0,
            agent: text(index, 'agent'),
            what: text(index, 'what'),
            cost: text(index, 'cost').isEmpty ? null : text(index, 'cost'),
          ),
      ];
    } finally {
      _kinetoMcpServerAsksDestroy(handle);
    }
  } finally {
    arena.releaseAll();
  }
}

/// The director allows or declines what an agent asked: a command then
/// runs, or the agent may connect. Touches only memory.
void decideKinetoAgentAsk(int id, {required bool allow}) {
  _assertAbiCompatible();
  _checkProjectResult(_kinetoMcpServerDecide(id, allow ? 1 : 0));
}
