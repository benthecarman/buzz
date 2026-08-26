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

export function zapAuthorSubscriptionFilter(
  pubkeys: readonly string[],
  since: number,
) {
  return {
    kinds: [KIND_BOLT12_ZAP],
    authors: [
      ...new Set(
        pubkeys.map((pubkey) => pubkey.trim().toLowerCase()).filter(Boolean),
      ),
    ],
    limit: 50,
    since,
  };
}

export function zapLiveSubscriptionFilters(
  recipientPubkeys: readonly string[],
  authorPubkeys: readonly string[],
  since: number,
  channelIds: readonly string[],
) {
  const received = zapSubscriptionFilter(recipientPubkeys, since);
  const sent = zapAuthorSubscriptionFilter(authorPubkeys, since);
  const channels = [...new Set(channelIds.map((id) => id.trim()))]
    .filter(Boolean)
    .sort();
  return [
    received,
    sent,
    ...channels.flatMap((channelId) => [
      { ...received, "#h": [channelId] },
      { ...sent, "#h": [channelId] },
    ]),
  ];
}
