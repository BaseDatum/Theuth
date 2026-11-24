import type { ReactNode } from "react";

export const DOCS = "/docs";

/** A link into the docs at /docs/<path>; paths follow the docs' own tree (getting-started, workspace, automation, reference). docs-cta makes it tap-sized when it stands alone. */
export function Doc({ path = "", className = "", children }: { path?: string; className?: string; children: ReactNode }) {
  return (
    <a href={path ? `${DOCS}/${path}` : DOCS} className={`docs-link ${className}`.trim()}>
      {children}
    </a>
  );
}
