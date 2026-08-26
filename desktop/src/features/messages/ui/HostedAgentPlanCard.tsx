import { Bot } from "lucide-react";
import * as React from "react";

import type { HostedAgentPlanMessage } from "@/features/messages/lib/hostedAgentZap";
import bitcoinIconUrl from "@/features/profile/assets/bitcoin.svg?inline";
import {
  Attachment,
  AttachmentAction,
  AttachmentActions,
  AttachmentContent,
  AttachmentDescription,
  AttachmentMedia,
  AttachmentTitle,
} from "@/shared/ui/attachment";
import type { MessageZapAction } from "./useMessageZap";

export function HostedAgentPlanCard({
  plan,
  zapAction,
}: {
  plan: HostedAgentPlanMessage;
  zapAction: MessageZapAction;
}) {
  const unavailable = !zapAction.canZap;
  const disabled = unavailable || zapAction.disabled;
  const buttonLabel = unavailable
    ? "Bitcoin unavailable"
    : zapAction.disabled
      ? "Buying…"
      : `Create agent (₿${plan.hourlyPriceSats})`;

  const handleBuy = React.useCallback(
    (event: React.MouseEvent<HTMLButtonElement>) => {
      event.preventDefault();
      event.stopPropagation();
      if (!disabled) zapAction.run();
    },
    [disabled, zapAction],
  );

  return (
    <Attachment
      className="mt-1 max-w-xl items-stretch gap-2.5 border-amber-500/30 bg-amber-500/5"
      data-testid="hosted-agent-plan-card"
      orientation="vertical"
      size="default"
    >
      <div className="flex min-w-0 items-start gap-3">
        <AttachmentMedia aria-hidden="true" className="text-amber-500">
          <Bot />
        </AttachmentMedia>
        <AttachmentContent>
          <AttachmentTitle>{plan.name}</AttachmentTitle>
          <AttachmentDescription className="overflow-visible whitespace-normal text-clip">
            A new isolated agent for ₿{plan.hourlyPriceSats} per hour. Data is
            retained for {plan.retentionDays} days after expiry.
          </AttachmentDescription>
          <dl className="mt-2 grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-xs text-muted-foreground">
            <dt className="font-medium text-foreground">Harness</dt>
            <dd className="break-words">{plan.harnessProfile}</dd>
            <dt className="font-medium text-foreground">Model</dt>
            <dd className="break-words">{plan.model}</dd>
          </dl>
          <details className="mt-2 text-xs text-muted-foreground">
            <summary className="cursor-pointer font-medium text-foreground">
              System prompt
            </summary>
            <pre className="mt-1 max-h-48 overflow-auto whitespace-pre-wrap rounded-md bg-muted/60 p-2 font-mono text-xs text-foreground">
              {plan.systemPrompt}
            </pre>
          </details>
        </AttachmentContent>
      </div>
      <AttachmentActions className="w-full">
        <AttachmentAction
          aria-label={buttonLabel}
          className="w-full gap-1.5 text-primary-foreground hover:text-primary-foreground disabled:text-primary-foreground"
          data-testid="buy-hosted-agent"
          disabled={disabled}
          onClick={handleBuy}
          size="sm"
          type="button"
          variant="default"
        >
          {!unavailable ? (
            <img
              alt=""
              aria-hidden="true"
              className="h-4 w-4"
              src={bitcoinIconUrl}
            />
          ) : null}
          {buttonLabel}
        </AttachmentAction>
      </AttachmentActions>
    </Attachment>
  );
}
