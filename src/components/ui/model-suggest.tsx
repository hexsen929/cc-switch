import * as React from "react";
import { ChevronDown } from "lucide-react";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

interface ModelSuggestProps {
  value: string;
  onChange: (value: string) => void;
  suggestions: string[];
  placeholder?: string;
  id?: string;
  className?: string;
}

function scoreSuggestion(suggestion: string, query: string): number {
  if (!query) return 0;
  const s = suggestion.toLowerCase();
  const q = query.toLowerCase();
  if (s === q) return 3;
  if (s.startsWith(q)) return 2;
  if (s.includes(q)) return 1;
  return 0;
}

function highlightMatch(text: string, query: string): React.ReactNode {
  if (!query) return text;
  const idx = text.toLowerCase().indexOf(query.toLowerCase());
  if (idx === -1) return text;
  return (
    <>
      {text.slice(0, idx)}
      <span className="font-semibold text-foreground">
        {text.slice(idx, idx + query.length)}
      </span>
      {text.slice(idx + query.length)}
    </>
  );
}

export function ModelSuggest({
  value,
  onChange,
  suggestions,
  placeholder,
  id,
  className,
}: ModelSuggestProps) {
  const [open, setOpen] = React.useState(false);
  const containerRef = React.useRef<HTMLDivElement>(null);
  const inputRef = React.useRef<HTMLInputElement>(null);

  const sorted = React.useMemo(() => {
    if (suggestions.length === 0) return [];
    const scored = suggestions.map((item) => ({
      value: item,
      score: scoreSuggestion(item, value),
    }));
    scored.sort((a, b) => {
      if (b.score !== a.score) return b.score - a.score;
      return a.value.localeCompare(b.value, "en-US");
    });
    return scored;
  }, [suggestions, value]);

  const showDropdown = open && sorted.length > 0;

  React.useEffect(() => {
    if (!showDropdown) return;
    const handleMouseDown = (event: MouseEvent) => {
      if (containerRef.current?.contains(event.target as Node)) return;
      setOpen(false);
    };
    document.addEventListener("mousedown", handleMouseDown);
    return () => document.removeEventListener("mousedown", handleMouseDown);
  }, [showDropdown]);

  React.useEffect(() => {
    if (!showDropdown) return;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [showDropdown]);

  const handleSelect = (selected: string) => {
    onChange(selected);
    setOpen(false);
    inputRef.current?.focus();
  };

  return (
    <div ref={containerRef} className="relative">
      <Input
        ref={inputRef}
        id={id}
        type="text"
        value={value}
        onChange={(event) => {
          onChange(event.target.value);
          setOpen(true);
        }}
        onFocus={() => setOpen(true)}
        placeholder={placeholder}
        autoComplete="off"
        className={cn("pr-8", className)}
      />
      {sorted.length > 0 && (
        <button
          type="button"
          tabIndex={-1}
          onMouseDown={(event) => event.preventDefault()}
          onClick={() => setOpen((prev) => !prev)}
          className="absolute right-2 top-1/2 -translate-y-1/2 text-muted-foreground transition-colors hover:text-foreground"
        >
          <ChevronDown
            className={cn("h-4 w-4 transition-transform", showDropdown && "rotate-180")}
          />
        </button>
      )}
      {showDropdown && (
        <div className="absolute z-50 mt-1 w-full animate-in fade-in-0 zoom-in-95 rounded-md border bg-popover text-popover-foreground shadow-md">
          <div className="max-h-[200px] overflow-y-auto p-1">
            {sorted.map(({ value: item, score }) => (
              <button
                key={item}
                type="button"
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => handleSelect(item)}
                className={cn(
                  "w-full cursor-pointer rounded-sm px-2 py-1.5 text-left text-sm transition-colors hover:bg-accent hover:text-accent-foreground",
                  score === 0 && "text-muted-foreground",
                )}
              >
                {highlightMatch(item, value)}
              </button>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
