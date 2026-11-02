# ADR 0035 — A Nextcloud at home: trust its certificate, or allow http:// on the home network

- Status: Accepted
- Date: 2026-11-02
- Amends: docs/05 (WebDAV mode was HTTPS-only, except on this machine)

## Context
WebDAV mode accepted only `https://` (plain `http` on loopback only), with the certificate
checked as a browser would. A tester runs Nextcloud at home under a name only his router's DNS
knows (`pve.example.lan` → `192.168.1.x`). Its certificate is the self-signed one the server
made at install, with no name in it. So nothing could get through:

- by name, rclone refused the certificate (`x509: certificate is not valid for any names`);
- by IP, the same (`doesn't contain any IP SANs`), and the hint said to use the name;
- with `http://`, the wizard refused the address.

Many home servers look like this: a self-signed certificate, or none at all on the LAN. A
browser lets its user click through. filebridge had no way out.

## Decision
Two opt-ins per destination, in the wizard's form ("Connection options") and as CLI flags.

- **Trust this server's certificate** (`skip_certificate_check`, on `Backend::Webdav` and in the
  Nextcloud settings). rclone gets `override.no_check_certificate=true` in that remote's
  connection string only. Other destinations keep checking. The same string works on the
  sidecar and on librclone. Bulk upload uses a second reqwest client that accepts any
  certificate, for these destinations only. It is offered for `https://` only.
- **Allow http://** (`plain_http`, in the Nextcloud settings). It is accepted only when the host
  is on the home network: loopback, private (10/8, 172.16/12, 192.168/16), link-local,
  Tailscale's 100.64/10 (encrypted by Tailscale anyway), and IPv6 ULA or link-local. A name has
  to resolve to such addresses only. The wizard resolves it when the destination is added.
- Both are said, as a warning, in the wizard's Address row and on the destination's card.
- A sign-in that fails on a certificate now says so in words: the certificate is self-signed or
  made for another name, and the user can trust it. The failed step carries
  `hint: "certificate"`, so the wizard opens the connection options. An IP address still gets
  the "use the name" advice. When the server broke off the handshake (`tls: internal error`,
  Tailscale serve without a name), there is no certificate to trust, so the option isn't
  offered.

## Consequences
- With the certificate unchecked, someone on the path could pose as the server and read the app
  password. With plain http, anyone on the network can read it. Both are the user's informed
  choice, worded so in pt and en. Neither is the default.
- The home-network check runs once, when the destination is added. A name later moved to a
  public address is not re-checked before each send.
- There is no certificate pinning. Stock rclone can't pin a fingerprint, and a pin checked only
  by filebridge's own client would leave rclone's connection unchecked anyway. A pin could come
  later if librclone's shim grows one.
- Immich stays HTTPS-only (ADR 0020). Its own wizard can take the same options if asked.
- `PROTOCOL_VERSION` 37.
