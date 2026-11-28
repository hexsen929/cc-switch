import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronDown } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import type { FetchedModel } from "@/lib/api/model-fetch";

export function ModelDropdown({
  models,
  onSelect,
  open: controlledOpen,
  onOpenChange,
  disabled,
}: {
  models: FetchedModel[];
  onSelect: (id: string) => void;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const [localOpen, setLocalOpen] = useState(false);
  const open = controlledOpen ?? localOpen;
  const setOpen = onOpenChange ?? setLocalOpen;

  // Group models by vendor; missing ownedBy falls back to "Other"
  const grouped = new Map<string, FetchedModel[]>();
  for (const model of models) {
    const vendor = model.ownedBy || "Other";
    const group = grouped.get(vendor);
    if (group) group.push(model);
    else grouped.set(vendor, [model]);
  }
  const vendors = [...grouped.keys()].sort();

  return (
    <Popover modal open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          type="button"
          disabled={disabled}
          variant="outline"
          size="icon"
          className="shrink-0"
          aria-label={t("providerForm.searchModelAriaLabel", {
            defaultValue: "Select model",
          })}
        >
          <ChevronDown className="h-4 w-4" />
        </Button>
      </PopoverTrigger>
      <PopoverContent
        align="end"
        sideOffset={4}
        collisionPadding={8}
        className="z-[200] w-72 p-0"
      >
        <Command
          label={t("providerForm.searchModelPlaceholder", {
            defaultValue: "Search models...",
          })}
        >
          <CommandInput
            placeholder={t("providerForm.searchModelPlaceholder", {
              defaultValue: "Search models...",
            })}
          />
          <CommandList className="max-h-64">
            <CommandEmpty>
              {t("providerForm.searchModelEmpty", {
                defaultValue: "No matching models.",
              })}
            </CommandEmpty>
            {vendors.map((vendor) => (
              <CommandGroup key={vendor} heading={vendor}>
                {grouped.get(vendor)?.map((m) => (
                  <CommandItem
                    key={m.id}
                    value={m.id}
                    // Expose the vendor name as a keyword so models can also be
                    // fuzzy-matched by vendor, not just by model id.
                    keywords={[m.ownedBy || "Other"]}
                    onSelect={() => {
                      onSelect(m.id);
                      setOpen(false);
                    }}
                  >
                    {m.id}
                  </CommandItem>
                ))}
              </CommandGroup>
            ))}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}
