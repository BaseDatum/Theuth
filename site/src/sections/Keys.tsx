import { Doc } from "./doc";

/**
 * The security story. Every sentence here traces to the as-built sharing, link, mesh, contacts and vault notes;
 * the edge's visibility is stated in full because the honesty is the pitch. No "zero knowledge", no "never leaves".
 */

/** One public share, hop by hop: who holds what, and where TLS starts and ends. */
const HOPS = [
  {
    role: "visitor",
    title: "A browser anywhere",
    body: "Opens https://demo.sky.tun.zip and checks an ordinary certificate. Nothing to install, no account.",
    tls: "TLS starts here",
  },
  {
    role: "sharing edge",
    title: "tun.zip, or your own",
    body: "Reads the hostname in the TLS hello and splices the connection into the tunnel your machine dialled out. It never presents a certificate and never decrypts.",
    tls: "ciphertext passes through",
  },
  {
    role: "your machine",
    title: "Laptop, desktop or server",
    body: "Holds the certificate key in its vault, terminates TLS, checks the password if there is one and serves your dev server or folder.",
    tls: "TLS ends here",
  },
];

const EDGE_SEES = [
  "Visitor IP addresses, kept 30 days for abuse handling",
  "The full hostname of every visit",
  "Your mesh id and its machines' public keys",
  "Byte counts and timing",
];

const EDGE_NEVER = [
  "The pages, the requests or the responses",
  "Passwords visitors type at your gate",
  "Your certificate's private key",
  "Your vault, sessions or files",
];

/** [name, how it works, who sees what] */
const CHANNELS: [string, string, string][] = [
  [
    "your machines",
    "Each machine has an Ed25519 identity and reaches the others over iroh QUIC: directly when it can, through a relay when it can't. Pair with a one-time string or a nearby code both screens confirm. The string carries the vault key, so treat it as a secret.",
    "The relay sees endpoint ids and IP addresses, not content. Point mesh.relay at your own, or at nothing for a LAN-only mesh.",
  ],
  [
    "phone and browser",
    "Noise on the WireGuard pattern: X25519 keys, AES-GCM, mutual authentication, forward secrecy. The pairing QR is the trust root; its secret works once and expires in five minutes. Revoke a lost phone and its next handshake is refused.",
    "The relay keeps no accounts and no database. It sees that devices are connected, when and how much, not what they say or which device is talking. A browser also trusts the site that serves its page; the desktop app and a self-hosted relay remove that.",
  ],
  [
    "other people",
    "Another person's theuth becomes a contact once a code derived from both keys is confirmed on both screens. You export the agents and chats they may reach; Ask first holds a contact's first message until you allow it. Remove is local and immediate.",
    "A contact gets what you export and nothing else: no vault, no replication, and their messages run under the approvals of the chat they reach.",
  ],
  [
    "secrets",
    "A vault sealed with XChaCha20-Poly1305, its key in your OS keychain. A tool call carries a placeholder; the value goes in after the approval card and before the tool runs, and is scrubbed from what comes back.",
    "The model sees names, notes and dates, never a value. The card and the transcript show the placeholder. The service you send a key to sees it, as it must.",
  ],
  [
    "publishing",
    "share_publish is human-only: you approve it in person, on a screen you own or in your own Slack, Signal or Telegram. Another user, the double or a policy switch cannot. Public sharing ships off on every machine.",
    "Anyone with a public link can read it; that is what public means. Add a password, keep the 8 hour expiry or press Stop.",
  ],
];

export function Keys() {
  return (
    <section
      id="keys"
      aria-labelledby="keys-heading"
      className="mx-auto max-w-[1400px] scroll-mt-24 px-6 pb-36 md:px-10"
    >
      <div className="md:w-[54%]">
        <h2 id="keys-heading" className="display text-[clamp(44px,5.4vw,76px)]">
          The keys stay home.
        </h2>
        <p className="mt-6 max-w-[34em] text-[17px] leading-relaxed text-fg-2">
          There is no theuth account and no cloud copy of your sessions: they
          live in ~/.theuth. Traffic between your machines, your phone and the
          people you work with is encrypted end to end. Publish a page to the
          world and TLS still ends on your own computer, so the edge in the
          middle forwards bytes it cannot read.
        </p>
        <p className="mt-4 max-w-[34em] text-[15px] leading-relaxed text-fg-3">
          Prompts go to the model provider you pick, as in any chat; run a
          local model and they stay on the machine too.
        </p>
      </div>

      <figure className="mt-14 border border-well bg-page/85 backdrop-blur-[2px]">
        <figcaption className="border-b border-well px-7 py-4 text-[14px] text-fg-3 md:px-9">
          A public share, hop by hop.
        </figcaption>
        <ol className="grid divide-y divide-well md:grid-cols-3 md:divide-x md:divide-y-0">
          {HOPS.map((h, i) => (
            <li
              key={h.role}
              className={`flex flex-col p-7 md:p-9 ${i === 1 ? "bg-raised/60" : ""}`}
            >
              <p className="index">{h.role}</p>
              <h3 className="mt-3 text-[22px] font-medium">{h.title}</h3>
              <p className="mt-4 flex-1 text-[15px] leading-relaxed text-fg-2">
                {h.body}
              </p>
              <p className="mt-6 font-mono text-[12.5px] text-fg md:hidden">
                {h.tls}
              </p>
            </li>
          ))}
        </ol>
        {/* The tunnel: one TLS session from the visitor's browser to your machine, drawn under all three hops so
            the edge visibly sits outside it. The hop cells carry the same words for screen readers and phones. */}
        <div aria-hidden className="hidden border-t border-well px-9 pt-7 pb-8 md:block">
          <div className="grid grid-cols-3 font-mono text-[13px]">
            <p className="text-center text-fg">TLS starts</p>
            <p className="text-center text-fg-3">the edge copies ciphertext</p>
            <p className="text-center text-fg">TLS ends</p>
          </div>
          <div className="relative mt-4 h-4">
            <div className="absolute inset-y-0 right-[16.667%] left-[16.667%] border border-fg-2" />
            <div className="absolute top-1/2 right-[16.667%] left-[16.667%] border-t border-dashed border-fg-3" />
            <div className="absolute -inset-y-1.5 left-[16.667%] w-px bg-fg" />
            <div className="absolute -inset-y-1.5 right-[16.667%] w-px bg-fg" />
          </div>
          <p className="mt-4 text-center text-[13px] text-fg-3">
            One TLS session, from the visitor's browser to your machine.
          </p>
        </div>
      </figure>

      <div className="-mt-px grid gap-px border border-well bg-well md:grid-cols-2">
        <div className="bg-page/85 p-7 backdrop-blur-[2px] md:p-9">
          <h3 className="text-[18px] font-medium">The edge sees</h3>
          <ul className="mt-5 divide-y divide-well/60 text-[15px] leading-relaxed text-fg-2">
            {EDGE_SEES.map((s) => (
              <li key={s} className="py-3">
                {s}
              </li>
            ))}
          </ul>
        </div>
        <div className="bg-page/85 p-7 backdrop-blur-[2px] md:p-9">
          <h3 className="text-[18px] font-medium">The edge never sees</h3>
          <ul className="mt-5 divide-y divide-well/60 text-[15px] leading-relaxed text-fg-2">
            {EDGE_NEVER.map((s) => (
              <li key={s} className="py-3">
                {s}
              </li>
            ))}
          </ul>
        </div>
      </div>

      <dl className="mt-14 border border-well bg-page/85 backdrop-blur-[2px]">
        <div className="hidden grid-cols-[10rem_1fr_1fr] gap-8 px-9 pt-9 pb-3 text-[12px] tracking-[0.12em] text-fg-3 uppercase md:grid">
          <span />
          <span>how it works</span>
          <span>who sees what</span>
        </div>
        <div className="divide-y divide-well/60">
          {CHANNELS.map(([name, how, who]) => (
            <div
              key={name}
              className="grid gap-2 px-7 py-6 md:grid-cols-[10rem_1fr_1fr] md:gap-8 md:px-9"
            >
              <dt className="font-mono text-[14px] text-fg">{name}</dt>
              <dd className="text-[14.5px] leading-relaxed text-fg-2">{how}</dd>
              <dd className="text-[14.5px] leading-relaxed text-fg-3">{who}</dd>
            </div>
          ))}
        </div>
      </dl>

      <div className="mt-10 md:w-[54%]">
        <p className="max-w-[34em] text-[15px] leading-relaxed text-fg-3">
          What this does not stop: whoever owns the edge's domain controls its
          DNS and could mint a certificate for your name. It would show in
          public Certificate Transparency logs; it is not prevented. Run the
          edge yourself, on a domain you own, and that party is you.
        </p>
        <p className="mt-4 flex flex-wrap gap-x-6">
          <Doc path="reference/security" className="docs-cta">
            Docs: the security model →
          </Doc>
          <Doc path="automation/sharing" className="docs-cta">
            Docs: sharing and the edge →
          </Doc>
        </p>
      </div>
    </section>
  );
}
