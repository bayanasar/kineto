import 'package:flutter_test/flutter_test.dart';
import 'package:kineto/native/kineto_engine.dart';

/// These tests fail when the Rust and Dart sides of the native contract are
/// changed apart: the ABI revision cannot catch a one-sided edit, because both
/// revisions are bumped by hand in the same change.
void main() {
  final contract = KinetoNativeContract.instance;

  test('project status codes match the native table name for name', () {
    expect(kinetoProjectStatusCodes, contract.statuses);
    for (final code in contract.statuses.values.where((code) => code != 0)) {
      expect(
        kinetoProjectStatusMessages,
        contains(code),
        reason: 'status $code has no message for the director',
      );
    }
  });

  test('status constants used for control flow match the native table', () {
    expect(KinetoProjectException.alreadyExists, contract.statuses['already_exists']);
    expect(KinetoProjectException.busy, contract.statuses['busy']);
    expect(KinetoProjectException.migrationRequired, contract.statuses['migration_required']);
    expect(KinetoProjectException.lockUnavailable, contract.statuses['lock_unavailable']);
    expect(KinetoProjectException.generationJob, contract.statuses['generation_job']);
    expect(
      KinetoProjectException.providerNotConfigured,
      contract.statuses['provider_not_configured'],
    );
    expect(KinetoProjectException.shotIntentMissing, contract.statuses['shot_intent_missing']);
    expect(KinetoProjectException.providerRefused, contract.statuses['provider_refused']);
  });

  test('video state codes match the native table', () {
    expect(
      <String>{for (final state in KinetoVideoState.values) state.wireName},
      contract.videoStates.keys.toSet(),
    );
    expect(KinetoProjectException.videoInFlight, contract.statuses['video_in_flight']);
  });

  test('image provider codes match the native table', () {
    expect(
      <String, int>{
        for (final kind in KinetoImageProviderKind.values) kind.wireName: kind.wireValue,
      },
      contract.imageProviders,
    );
  });

  test('shot direction wire codes match the native table', () {
    expect(
      <String, int>{
        for (final direction in KinetoShotDirection.values)
          direction.wireName: direction.wireValue,
      },
      contract.directions,
    );
  });

  test('a snapshot packed natively decodes to the values it was packed from', () {
    final sample = KinetoShotSnapshot.fromBits(contract.snapshotSample);

    expect(sample.generated, isTrue);
    expect(sample.locked, isFalse);
    expect(sample.stale, isTrue);
    expect(sample.migrationPending, isTrue);
    expect(sample.hasInvalidHashes, isTrue);
    expect(sample.candidateCount, 3);
    expect(sample.selectedIndex, 2);
    expect(sample.direction, KinetoShotDirection.tension);
    expect(sample.generation, 7);
    expect(sample.supersededCount, 18);
    expect(sample.videoState, KinetoVideoState.stale);
    expect(
      contract.snapshotFields,
      containsAll(<String>[
        'generated',
        'locked',
        'stale',
        'migration_pending',
        'invalid_hashes',
        'candidate_count',
        'selected_plus_one',
        'direction',
        'generation_revision',
        'superseded_count',
        'video_state',
      ]),
    );
  });
}
