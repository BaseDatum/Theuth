import { Doc } from "./doc";

/** share_publish as it runs: figures are from the first live run on tun.zip (2026-11-23), not a benchmark. */
const STEPS = [
  {
    step: "the agent asks",
    body: "share_publish names a source (a dev server on a loopback port or a folder of static files), a label, an expiry and the access. Public sharing has to be on for that machine first; it ships off.",
  },
  {
    step: "you approve",
    body: "The card shows the exact URL, source, expiry and access. Only you can press Allow, in person: in the app, the terminal or your own Slack, Signal or Telegram. No answer in five minutes is a no.",
  },
  {
    step: "it is live",
    body: "The URL answers over https with a Let's Encrypt wildcard certificate for your name, its key made on your own machines. In the first live run a certified name went from publish to first visit in 385 ms; a new name's first certificate took about two and a half minutes.",
  },
  {
    step: "it ends",
    body: "Links expire after 8 hours unless you pick 1 hour, 24 hours or 7 days. A red strip says you are public; Stop ends one share, Stop all ends every share in the mesh, from the app, the terminal or Slack.",
  },
];

export function Publish() {
  return (
    <section className="mx-auto max-w-[1400px] px-6 pb-36 md:px-10">
      <div className="md:w-[54%]">
        <h2 className="display text-[clamp(44px,5.4vw,76px)]">
          Publish from your laptop.
        </h2>
        <p className="mt-6 max-w-[34em] text-[17px] leading-relaxed text-fg-2">
          Ask for a link and theuth puts the thing you are building on the
          public internet at a real https address, served from your own
          machine. It works behind NAT: your machine dials out to the edge, so
          no port is opened and no account is made. Pick a name like sky.tun.zip
          or take a random one; any machine in your mesh can publish under it.
        </p>
      </div>

      <ol className="mt-14 grid divide-y divide-well border border-well bg-page/85 backdrop-blur-[2px] md:grid-cols-4 md:divide-x md:divide-y-0">
        {STEPS.map((s) => (
          <li key={s.step} className="p-7 md:p-9">
            <h3 className="font-mono text-[14px] text-fg">{s.step}</h3>
            <p className="mt-4 text-[14.5px] leading-relaxed text-fg-2">{s.body}</p>
          </li>
        ))}
      </ol>

      <div className="mt-10 md:w-[54%]">
        <pre className="overflow-x-auto border border-well bg-well/60 p-4 font-mono text-[12.5px] leading-6 text-fg-2">
          {
            "share_publish\n  source: { port: 5173 }\n  label:  demo\n  expiry: 8h\n  access: password\n\n# you press Allow, then\nhttps://demo.sky.tun.zip"
          }
        </pre>
        <p className="mt-4 max-w-[34em] text-[14px] leading-relaxed text-fg-3">
          With a password, theuth generates it and shows it to you; the model
          never gets it. Visitors see a short interstitial before your page.
          The machine has to stay awake: this is a tunnel, not hosting.
        </p>
        <Doc path="automation/sharing" className="docs-cta mt-4">
          Docs: sharing →
        </Doc>
      </div>
    </section>
  );
}
