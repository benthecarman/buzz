import { KIND_BOLT12_ZAP } from "@/shared/constants/kinds";

export function zapSubscriptionFilter(
  pubkeys: readonly string[],
  since: number,
) {
  return {
    kinds: [KIND_BOLT12_ZAP],
    "#p": [
      ...new Set(
        pubkeys.map((pubkey) => pubkey.trim().toLowerCase()).filter(Boolean),
      ),
    ],
    limit: 50,
    since,
  };
}
