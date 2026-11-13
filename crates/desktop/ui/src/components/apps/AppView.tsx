import { Suspense, lazy } from "react";
import type { Artifact } from "@/rpc/bindings/method.artifact.output"
import type { AppDraft } from "@/store/sessions";

/** The OpenUI runtime, component library and its CSS load on the first preview/open (46 §7), not with the chat. */
const load = () => import("./AppHost");
const Host = lazy(load);

/** Start loading the runtime now (a direct `app` call is pending in a visible session) so it is warm when the
 * first renderable source arrives; the chunk is cached, so repeated calls cost nothing. */
export function preloadAppView(): void {
  void load().catch(() => undefined);
}

export type AppViewProps = {
  session: string;
  /** The committed record to run; absent while only a draft exists. Changing it within one mounted view is a new generation. */
  artifact?: Artifact;
  /** The pending/failed direct `app` call this view previews (46 §8.1). */
  draft?: AppDraft;
  className?: string;
};

/** One app view: the chat panel and the explorer both render through it; `AppHost` carries the runtime. */
export function AppView(props: AppViewProps) {
  return (
    <div className={props.className}>
      <Suspense
        fallback={
          <div className="m-6 h-64 animate-pulse rounded-xl bg-layer-2" />
        }
      >
        <Host {...props} />
      </Suspense>
    </div>
  );
}
