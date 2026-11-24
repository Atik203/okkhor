import * as React from "react";
import { cn } from "../../lib/utils";

export interface TextareaProps
  extends React.TextareaHTMLAttributes<HTMLTextAreaElement> {}

/**
 * Multi-line field matching `Input` (border, surface, disabled state). The
 * global `:focus-visible` rule supplies the accent focus ring, exactly as it
 * does for inputs and buttons (index.css).
 */
const Textarea = React.forwardRef<HTMLTextAreaElement, TextareaProps>(
  ({ className, rows = 5, ...props }, ref) => (
    <textarea
      ref={ref}
      rows={rows}
      className={cn(
        "flex min-h-20 w-full resize-y rounded-md border border-neutral-300 bg-white px-3 py-2 text-sm shadow-sm transition-colors placeholder:text-neutral-400 disabled:cursor-not-allowed disabled:opacity-50 dark:border-neutral-700 dark:bg-neutral-900 dark:placeholder:text-neutral-500",
        className,
      )}
      {...props}
    />
  ),
);
Textarea.displayName = "Textarea";

export { Textarea };
