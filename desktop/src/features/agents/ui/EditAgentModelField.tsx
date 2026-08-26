import { cn } from "@/shared/lib/cn";
import { Input } from "@/shared/ui/input";

import {
  PERSONA_FIELD_CONTROL_CLASS,
  PERSONA_FIELD_SHELL_CLASS,
  PERSONA_LABEL_OPTIONAL_CLASS,
  type PersonaDropdownOption,
} from "./agentConfigOptions";
import { PersonaDropdownField } from "./PersonaDropdownField";

/** Model dropdown with an optional custom-ID override for the edit dialog. */
export function EditAgentModelField({
  customValue,
  disabled,
  discoveryLoading,
  onCustomValueChange,
  onValueChange,
  options,
  required,
  selectValue,
  showCustomInput,
  statusMessage,
}: {
  customValue: string;
  disabled: boolean;
  discoveryLoading: boolean;
  onCustomValueChange: (value: string) => void;
  onValueChange: (value: string) => void;
  options: readonly PersonaDropdownOption[];
  required: boolean;
  selectValue: string;
  showCustomInput: boolean;
  statusMessage?: string | null;
}) {
  return (
    <div className="space-y-1.5">
      <label
        className="text-sm font-medium text-foreground"
        htmlFor="edit-agent-model"
      >
        Model
        {required ? (
          <span className="ml-1 text-destructive" aria-hidden="true">
            *
          </span>
        ) : (
          <span className={PERSONA_LABEL_OPTIONAL_CLASS}>Optional</span>
        )}
      </label>
      <PersonaDropdownField
        disabled={disabled || discoveryLoading}
        id="edit-agent-model"
        onValueChange={onValueChange}
        options={options}
        placeholder="Default model"
        value={selectValue}
      />
      {showCustomInput ? (
        <div
          className={cn(
            "mt-2 flex min-h-11 items-center px-3",
            PERSONA_FIELD_SHELL_CLASS,
          )}
        >
          <Input
            aria-label="Custom model ID"
            autoCorrect="off"
            className={cn(
              "h-8 px-0 py-0 leading-6",
              PERSONA_FIELD_CONTROL_CLASS,
            )}
            disabled={disabled}
            id="edit-agent-custom-model"
            onChange={(event) => onCustomValueChange(event.target.value)}
            placeholder="Custom model ID"
            value={customValue}
          />
        </div>
      ) : null}
      {statusMessage ? (
        <p className="text-xs text-muted-foreground">{statusMessage}</p>
      ) : null}
    </div>
  );
}
