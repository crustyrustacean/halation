# Deploying Halation to a DigitalOcean droplet

Target shape: one droplet running the compose stack (app + postgres +
Caddy), DNS on Cloudflare, media in Cloudflare R2, TLS via Let's
Encrypt handled automatically by Caddy.

```
Internet ──> :443 Caddy ──> app:8000 ──> db:5432 (compose network)
                                  └──> Cloudflare R2 (media)
```

## 0. Prerequisites

- A domain (or subdomain) on Cloudflare DNS — any record you control
- An SSH key on the machine you deploy from (`ssh-keygen -t ed25519`
  if you don't have one)
- The four R2 values from Cloudflare (bucket, endpoint, access key,
  secret key)
- Your local machine with Docker (the image is built there and
  transferred — the droplet never compiles)

## 1. Create the droplet

1. Log in to DigitalOcean → green **Create** button (top right) → **Droplets**
2. **Region**: closest to you (e.g. San Francisco)
3. **Image**: OS → **Ubuntu 24.04 (LTS) x64**
4. **Size**: Shared CPU → Basic → **Regular**
   - $6/mo (1 GB) works; **$12/mo (2 GB)** recommended for headroom
5. **Authentication**: SSH Key → **New SSH Key** → paste your public key (see below)

   > **What is this key?** It is the *personal* key from the machine you
   > SSH from — it lets **you** reach the droplet. A second, separate key
   > (a GitHub **deploy key**) is created on the droplet in Part 4 so the
   > droplet can reach GitHub. Two keys, two directions.

   On your own machine, check whether you already have a key:

   ```bash
   ls ~/.ssh/id_ed25519.pub 2>/dev/null || ls ~/.ssh/id_rsa.pub 2>/dev/null
   ```

   - If one exists: use it — `cat` it and copy the line.
   - If not, create one (on **your machine**, not the droplet):

     ```bash
     ssh-keygen -t ed25519
     # accept the default path; a passphrase is recommended here
     cat ~/.ssh/id_ed25519.pub
     ```

   A public key is one long line starting with `ssh-ed25519` (or
   `ssh-rsa`) — copy **the entire line**.
6. Optional but recommended: tick **Backups** (weekly, +20%)
7. Hostname: `halation-prod-1` → **Create Droplet**
8. Copy the **IPv4 address** from the droplet page

## 2. Point DNS at the droplet

Cloudflare dashboard → your domain → **DNS** → **Add record**:

- Type `A`, Name `photos` (or `@` for the root), IPv4 = droplet IP
- Proxy status: **DNS only (grey cloud)** for now — Caddy must face
  Let's Encrypt directly. After HTTPS works you may switch to proxied;
  then set Cloudflare SSL/TLS mode to **Full (strict)**.

## 3. Server setup (over SSH)

```bash
ssh root@<droplet-ip>
curl -fsSL https://get.docker.com | sh
```

### 3.5 Create an admin user (recommended)

A one-time five minutes: it removes `root` — the most-targeted account
name on the internet — from direct SSH exposure and adds an
accident-proofing layer. (Honest calibration: with key-only auth and a
single admin this is defense-in-depth, not a wall — `docker` group
membership is root-equivalent.)

```bash
adduser --gecos "" jeff                     # sets the sudo password
usermod -aG sudo,docker jeff
mkdir -p /home/jeff/.ssh
cp /root/.ssh/authorized_keys /home/jeff/.ssh/authorized_keys
chown -R jeff:jeff /home/jeff
chmod 700 /home/jeff/.ssh
chmod 600 /home/jeff/.ssh/authorized_keys
```

From your laptop, verify **before** hardening anything:

```bash
ssh jeff@<droplet-ip>          # same public key, key auth just works
sudo docker compose ls         # entered from /opt/halation
```

Optional, once verified: disable direct root SSH
(`echo "PermitRootLogin no" | sudo tee /etc/ssh/sshd_config.d/99-hardening.conf`
then `sudo systemctl restart ssh`). Keep a DO web console open as
break-glass while you do it. From here on, connect as
`ssh jeff@<droplet-ip>`.

## 4. Get the code (deploy key)

The repo is private, so the droplet needs GitHub to trust it. The
standard mechanism is a **deploy key**: an SSH keypair that lives on
the droplet, is registered with this one repository, and (by default)
is read-only.

### 4a. Generate the keypair — run this **on the droplet**

```bash
ssh-keygen -t ed25519 -f ~/.ssh/halation_deploy -N ""
```

- `-t ed25519` — the key algorithm (modern, short, strong)
- `-f ~/.ssh/halation_deploy` — where the **private** half is written
- `-N ""` — no passphrase (acceptable for a read-only, repo-scoped key
  on a server you control; a passphrase would require ssh-agent setup)

Two files appear:

- `~/.ssh/halation_deploy` — the **private** half. It never leaves the
  droplet and is never pasted anywhere.
- `~/.ssh/halation_deploy.pub` — the **public** half. This is the one
  you give to GitHub.

### 4b. Show the public key

```bash
cat ~/.ssh/halation_deploy.pub
```

One single line, starting with `ssh-ed25519` and ending with a comment.
Copy **the whole line** — everything on it, no trailing newline.

### 4c. Register it with GitHub

1. Browser → the `halation` repo on GitHub → **Settings** →
   **Deploy keys** (left sidebar, near the bottom) → **Add deploy key**
2. **Title**: `halation-prod-1` (or anything recognizable)
3. **Key**: paste the full line from 4b
4. Leave **"Allow write access"** UNCHECKED — read-only is least
   privilege, and this server only ever pulls
5. **Add key**

### 4d. Teach git on the droplet to use this key

Without this step, `git clone` tries your droplet's default keys and
fails with `Permission denied (publickey)`: SSH never goes fishing for
keys with custom names — it offers only the default set
(`id_ed25519`, `id_rsa`, ...) unless told otherwise.

The command is a heredoc — shell for "append the following lines to
this file" — and it adds one entry to `~/.ssh/config`, the SSH
client's address book: *"when connecting to the host `github.com`,
offer the private key at `~/.ssh/halation_deploy`."* (`IdentityFile`
always names the private half; ssh derives and offers the public half
itself — the private key never leaves the machine.)

```bash
cat >> ~/.ssh/config <<'EOF'
Host github.com
  IdentityFile ~/.ssh/halation_deploy
EOF
```

Prefer doing it by hand? `nano ~/.ssh/config` and add the same two
lines at the bottom — identical result. (`>>` appends; never `>` on a
config you already have.)

### 4e. Verify, then clone

```bash
ssh -T git@github.com
# expected success message:
# "Hi crustyrustacean/halation! You've successfully authenticated,
#  but GitHub does not provide shell access."

git clone -b trunk git@github.com:crustyrustacean/halation.git /opt/halation
cd /opt/halation
```

(Files from the clone are already owned by `jeff` — you cloned as jeff. The `chown` from the earlier draft is unnecessary and gone.)

## 5. Configure

Generate secrets locally, then write `.env`:

```bash
openssl rand -hex 24   # POSTGRES_PASSWORD
openssl rand -hex 48   # HALATION_SESSION_SIGNING_KEY
```

```bash
cat > .env <<'EOF'
DOMAIN=halation.photos
POSTGRES_PASSWORD=<from openssl above>
HALATION_SESSION_SIGNING_KEY=<from openssl above>
APP_STORAGE__BACKEND=s3
APP_STORAGE__R2__BUCKET=<your bucket>
APP_STORAGE__R2__ENDPOINT=https://<account-id>.r2.cloudflarestorage.com
APP_STORAGE__R2__ACCESS_KEY=<r2 access key id>
APP_STORAGE__R2__SECRET_KEY=<r2 secret access key>
APP_GEOCODE__ENABLED=true
EOF
chmod 600 .env
```

## 6. Transfer the app image (built for the droplet's architecture)

From the dev machine (`~/dev/crustyrustacean/halation`):

```bash
docker build --platform linux/amd64 -t halation:latest .
docker save halation:latest | gzip | ssh jeff@<droplet-ip> 'gunzip | docker load'
```

The droplet never compiles — the image arrives ready to run.

> **Apple Silicon / ARM gotcha:** Docker Desktop on an Apple Silicon Mac
> builds `linux/arm64` images by default. Loading an arm64 image onto an
> x86-64 droplet boots fine and then dies at start with
> `exec format error` in a restart loop. The `--platform linux/amd64`
> flag above is what prevents it. Verify after transfer:
> `docker image inspect halation:latest --format '{{.Os}}/{{.Architecture}}'`
> → `linux/amd64`.
>
> The cross-compile runs under emulation, so it is slower than a native
> build — let it run. If it is truly painful, the alternatives are a
> temporarily larger droplet to build on, or a CI job that builds and
> pushes the image for you.

## 7. Launch

```bash
cd /opt/halation
docker compose up -d
docker compose ps                    # db, app, caddy: all running
docker compose exec app curl -sf http://127.0.0.1:8000/health_check
```

The app creates and migrates its database automatically on first boot.

## 8. Verify

- `https://<your-domain>` — the feed page (certificate issued on first visit)
- Register a user, log in, upload a photo
- Check the image objects appear in the Cloudflare R2 bucket
- `GET /api/v1/health` should report `"database": "ok"` and
  `"storage": "ok"`

## 9. Firewall (recommended)

DigitalOcean → **Networking → Firewalls → Create Firewall**:

- Inbound rule 1: **SSH** — TCP 22 (DO's quick-add SSH template opens
  ONLY port 22 — that alone blocks the whole site)
- Inbound rule 2: **HTTP** — TCP 80
- Inbound rule 3: **HTTPS** — TCP 443
- Apply to `halation-prod-1`

Verify from outside the droplet: `curl -sI https://<domain>` → 200.

This sits at the hypervisor, so it can't be bypassed by published
Docker ports.

## 10. Maintenance

### Media key layout & the backfill bin

Media objects live at owner-scoped keys in the R2 bucket:

```
{owner_id}/{media_id}/original.{ext}   ← owner-private (carries EXIF)
{owner_id}/{media_id}/{thumb|medium|large}.jpg
```

Objects written before partitioning (2026-09) still sit at legacy
`{media_id}/…` keys until the one-off backfill runs. The deploy image ships
`backfill_media_keys` alongside the app; it copies every legacy object to
its owner-scoped key and repoints the database rows (copy-then-update in a
single transaction, legacy objects left in place, safe to re-run):

```bash
cd /opt/halation
docker compose exec app /app/backfill_media_keys --dry-run   # preview
docker compose exec app /app/backfill_media_keys             # migrate
```

Afterwards the legacy objects are unreferenced duplicates — delete them in
the Cloudflare R2 dashboard once the site verifies clean.

### Deploy an update

- **Deploy an update**: build locally, `docker save | ssh … docker load`
  (step 6), then on the droplet `cd /opt/halation && docker compose up -d`
- **Logs**: `docker compose logs -f app`
- **Database backup** (weekly cron, `crontab -e` as root):
  ```bash
  0 3 * * 0 docker exec halation-db-1 pg_dump -U postgres halation | gzip > /root/backups/halation-$(date +\%F).sql.gz
  ```
  (`mkdir -p /root/backups` first.) Droplet backups via the DO toggle
  cover the rest.
- **Container names** under this compose project: `halation-app-1`,
  `halation-db-1`, `halation-caddy-1`.

### If the app crash-loops with "previously applied but has been modified"

This is not a data problem and nothing is corrupted. It happens when the
sqlx major version changes, because **sqlx 0.9 changed the migration
checksum algorithm** (SHA-256 → SHA-384). Every checksum recorded by an
older sqlx then reads as "modified" to the new binary, and the app refuses
to boot rather than guessing.

You hit this deploying 0.14.0, which carried the 0.8 → 0.9 upgrade. Tell-tale
sign: `SELECT length(checksum) FROM _sqlx_migrations` is 32 for a database
written by sqlx 0.8, 48 by 0.9.

**Roll back first if you have not already.** Get the site back on the old
image before you diagnose anything:

```bash
cd /opt/halation && docker compose down
docker tag <previous-image-id> halation:latest   # e.g. 7a37fc19ce91
docker compose up -d
```

Then, deliberately, with a backup taken:

```sh
# 1. Back up. Do this even though it feels like a formality.
docker exec halation-db-1 pg_dump -U postgres -d halation > /opt/halation/backups/pre-$(date +%Y%m%d-%H%M%S).sql

# 2. Get the checksums the NEW sqlx expects, from a scratch database.
#    Do not compute these yourself — let the new binary produce them.
docker run -d --name ckp3 -e POSTGRES_PASSWORD=postgres -e POSTGRES_DB=chk postgres:17
IP=$(docker inspect ckp3 --format '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}')
docker run --rm --platform linux/amd64 --entrypoint sh \
  -e APP_DATABASE__HOST=$IP -e APP_DATABASE__PORT=5432 \
  -e APP_DATABASE__USERNAME=postgres -e APP_DATABASE__PASSWORD=postgres \
  -e APP_DATABASE__DATABASE_NAME=chk -e APP_STORAGE__BACKEND=memory \
  halation:latest -c '/app/halation & sleep 25; kill %1'
docker exec ckp3 psql -U postgres -d chk -tAc \
  "SELECT version||E'\t'||encode(checksum,'hex') FROM _sqlx_migrations ORDER BY version;" \
  > /tmp/new.tsv

# 3. PROVE the migrations produce the schema you already have, before
#    touching anything. This is the step that rules out a migration file
#    having been edited after it was applied — otherwise you would be
#    papering over real schema drift instead of fixing a checksum format.
docker exec ckp3 pg_dump -U postgres -d chk --schema-only --no-owner --no-privileges \
  | grep -v restrict | sort > /tmp/schema_new.txt
docker exec halation-db-1 pg_dump -U postgres -d halation --schema-only --no-owner --no-privileges \
  | grep -v restrict | sort > /tmp/schema_prod.txt
diff /tmp/schema_new.txt /tmp/schema_prod.txt && echo "IDENTICAL - safe to proceed"

# 4. Rewrite the checksums, guarded, in one transaction. The guard aborts
#    unless there are exactly the expected migrations, all successful, all
#    the same length — so a surprise rolls back rather than half-applying.
#    (Generate the UPDATE lines from /tmp/new.tsv; 8 rows for this schema.)
docker exec -i halation-db-1 psql -U postgres -d halation -v ON_ERROR_STOP=1 -1 -f - < fix.sql

docker rm -f ckp3
```

**Do not skip step 3.** The whole reason this is safe is that the fresh
database and production agree structurally. Without that check, a modified
migration file would produce a "checksum mismatch" that looks identical to
a version-boundary mismatch, and rewriting the checksums would make a
genuine schema difference permanent and invisible.

After the rewrite, `docker compose up -d` boots normally. This is a
one-time cost per sqlx major version — after 0.9 is the only version in
play it cannot recur.
