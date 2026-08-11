import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { splitOutgoingTags } from "../messages/lib/imetaMediaMarkdown.ts";
import {
  agentRuntimePackChargeSats,
  agentRuntimePackRequired,
  claimableReservation,
  runtimeReservationMessageTag,
  spendableMs,
} from "./runtimeContract.ts";

// The identical bytes the Rust side pins in
// desktop/src-tauri/src/commands/agent_runtime.rs: that test proves the
// agent_runtime_get_status command emits exactly `status`; these tests prove
// that given exactly `status`, the web layer derives `expectedInvocation`.
// A drift on either side of the Tauri IPC fails a test, not a live checkout.
const fixture = JSON.parse(
  readFileSync(
    new URL("../../../fixtures/agent-runtime-contract.json", import.meta.url),
    "utf8",
  ),
);

test("contract: the fixture status yields a claimable reservation", () => {
  const reservation = claimableReservation(fixture.status);
  assert.ok(reservation, "open reservation must be claimable");
  assert.equal(
    reservation.reservationEventId,
    fixture.status.openReservation.reservationEventId,
  );
});

test("contract: spendable credit counts the locked cap", () => {
  assert.equal(
    spendableMs(fixture.status),
    fixture.expectedInvocation.spendableMs,
  );
  // The locked remainder is below a fresh 15-minute pack, so a repeat
  // purchase at this balance would charge the full pack.
  assert.equal(
    agentRuntimePackRequired(fixture.expectedInvocation.spendableMs, 15),
    true,
  );
  assert.equal(
    agentRuntimePackChargeSats(
      fixture.expectedInvocation.spendableMs,
      15,
      fixture.status.pricing.rateSatsPerMinute,
    ),
    300,
  );
});

test("contract: the status derives the exact pinned invocation tags", () => {
  const reservation = claimableReservation(fixture.status);
  const tag = runtimeReservationMessageTag(
    fixture.agentPubkey,
    reservation.reservationEventId,
  );
  assert.deepEqual([tag], fixture.expectedInvocation.runtimeTags);
});

test("contract: the invocation tags ride the runtime send channel", () => {
  // The same routing the send path applies at the Tauri boundary: the tag
  // must land on the validated runtimeTags argument, never the imeta channel
  // (whose Rust guard would reject the whole paid message).
  const { mediaTags, runtimeTags } = splitOutgoingTags(
    fixture.expectedInvocation.runtimeTags,
  );
  assert.deepEqual(runtimeTags, fixture.expectedInvocation.runtimeTags);
  assert.deepEqual(mediaTags, []);
});
