import Link from "next/link";
import { LogoMark } from "@/components/logo";

export default function HomePage() {
  return (
    <div className="flex flex-col items-center justify-center text-center flex-1 gap-6 px-4 py-16">
      <LogoMark className="size-24" />
      <h1 className="text-4xl font-bold tracking-tight">
        Nex<span className="text-[#16dc9f]">Spec</span>
      </h1>
      <p className="max-w-xl text-fd-muted-foreground">
        An in-process context engine for AI coding agents. One static Rust
        binary that indexes code and specs into a graph, searches it, and serves
        token-budgeted context.
      </p>
      <div className="flex gap-3">
        <Link
          href="/docs"
          className="rounded-lg bg-[#16dc9f] px-5 py-2 font-medium text-black hover:opacity-90"
        >
          Read the docs
        </Link>
        <Link
          href="/docs/getting-started"
          className="rounded-lg border border-fd-border px-5 py-2 font-medium hover:bg-fd-accent"
        >
          Getting started
        </Link>
      </div>
    </div>
  );
}
