import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { splitOutgoingTags } from "../messages/lib/imetaMediaMarkdown.ts";
import { activeAccessZap, runtimeZapMessageTag } from "./runtimeContract.ts";

const fixture = JSON.parse(
  readFileSync(
    new URL("../../../fixtures/agent-runtime-contract.json", import.meta.url),
    "utf8",
  ),
);

test("contract: the fixture status yields an active access zap", () => {
  const zap = activeAccessZap(
    fixture.status,
    fixture.status.accessZap.createdAt,
  );
  assert.deepEqual(zap, fixture.status.accessZap);
});

test("contract: access expires after the published window", () => {
  assert.equal(
    activeAccessZap(fixture.status, fixture.status.accessZap.validUntil + 1),
    null,
  );
});

test("contract: the access zap yields the exact invocation tag", () => {
  const tag = runtimeZapMessageTag(
    fixture.agentPubkey,
    fixture.status.accessZap.zapEventId,
  );
  assert.deepEqual([tag], fixture.expectedInvocation.runtimeTags);
});

test("contract: the invocation tag uses the runtime send channel", () => {
  const { mediaTags, runtimeTags } = splitOutgoingTags(
    fixture.expectedInvocation.runtimeTags,
  );
  assert.deepEqual(runtimeTags, fixture.expectedInvocation.runtimeTags);
  assert.deepEqual(mediaTags, []);
});
