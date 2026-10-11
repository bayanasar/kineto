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
