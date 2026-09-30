import { Oxanium } from "next/font/google";
import { cn } from "@/lib/cn";

const wordmark = Oxanium({ subsets: ["latin"], weight: "600" });

/** NexSpec mark: white parts turn black in light mode, green stays. */
export function LogoMark({ className }: { className?: string }) {
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      viewBox="346 333 199 199"
      fill="none"
      aria-hidden="true"
      className={cn("size-6 shrink-0", className)}
    >
      <path
        d="M349 337 C400 358 442 386 475 413 V469 C430 437 392 408 349 376 Z"
        fill="#16dc9f"
      />
      <path
        className="fill-black dark:fill-white"
        d="M349 395 L394 427 V520 Q394 528 386 528 H357 Q349 528 349 520 Z"
      />
      <path
        className="fill-black dark:fill-white"
        d="M497 340 H533 Q541 340 541 348 V521 Q541 529 533 529 Q526 529 521 523 L497 494 Z"
      />
    </svg>
  );
}

export function Logo() {
  return (
    <span className="inline-flex items-center gap-2">
      <LogoMark />
      <span className={cn(wordmark.className, "text-lg tracking-wide")}>
        Nex<span className="text-[#16dc9f]">Spec</span>
      </span>
    </span>
  );
}
