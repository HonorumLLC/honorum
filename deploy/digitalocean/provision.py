#!/usr/bin/env python3
"""Plan/apply the small production VPS and managed PostgreSQL."""
import argparse
import ipaddress
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]
NAME = "honorum-production"
REGION = "nyc3"


def api(*args):
    return json.loads(subprocess.check_output(["doctl", "--context", "honorum", *args, "--output", "json"], text=True))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apply", action="store_true")
    parser.add_argument("--ssh-cidr", required=True)
    args = parser.parse_args()
    cidr = str(ipaddress.ip_network(args.ssh_cidr, strict=False))
    if ipaddress.ip_network(cidr).prefixlen != 32:
        parser.error("Use the administrator's single IPv4 /32 address")
    print(json.dumps({"name": NAME, "region": REGION, "droplet": "s-1vcpu-2gb",
        "postgres": "db-s-1vcpu-1gb", "postgres_version": "17", "daily_droplet_backups": True,
        "estimated_monthly_usd": 30.75, "ssh_cidr": cidr}, indent=2), flush=True)
    if not args.apply:
        return
    account = api("account", "get")
    if account.get("team", {}).get("uuid") != "99c5aa1f-fdbd-485b-b75f-f4a6309fa497":
        raise SystemExit("Selected DigitalOcean account does not match the reviewed production account")
    size = next(s for s in api("compute", "size", "list") if s["slug"] == "s-1vcpu-2gb")
    if not size["available"] or REGION not in size["regions"] or size["price_monthly"] > 12:
        raise SystemExit("Droplet availability or price changed; review the plan")
    out = ROOT / "artifacts/production"
    out.mkdir(parents=True, exist_ok=True, mode=0o700)

    def save(name, data):
        path = out / name
        path.write_text(json.dumps(data, indent=2) + "\n")
        path.chmod(0o600)

    # A recorded resource may be resumed. An unrecorded name collision is not
    # adopted, avoiding accidental changes to another deployment or duplicates.
    def existing(items, record):
        found = next((i for i in items if i["name"] == NAME), None)
        if found:
            path = out / record
            if not path.exists() or json.loads(path.read_text())["id"] != found["id"]:
                raise SystemExit(f"Unrecorded {NAME} resource exists; inspect it before retrying")
        return found

    tags = api("compute", "tag", "list")
    if not any(t["name"] == NAME for t in tags):
        api("compute", "tag", "create", NAME)
    vpc = existing(api("vpcs", "list"), "vpc.json")
    if not vpc:
        vpc = api("vpcs", "create", "--name", NAME, "--region", REGION, "--ip-range", "10.124.0.0/20")[0]
        save("vpc.json", vpc)
    public_key = (Path.home() / ".ssh/honorum_digitalocean.pub").read_text().strip()
    if not public_key.startswith("ssh-ed25519 "):
        raise SystemExit("Expected an Ed25519 public key")
    key = next((k for k in api("compute", "ssh-key", "list") if k["public_key"].split()[:2] == public_key.split()[:2]), None)
    if not key:
        key = api("compute", "ssh-key", "import", NAME, "--public-key-file", str(Path.home() / ".ssh/honorum_digitalocean.pub"))[0]
    firewall = existing(api("compute", "firewall", "list"), "firewall.json")
    if not firewall:
        firewall = api("compute", "firewall", "create", "--name", NAME, "--tag-names", NAME,
            "--inbound-rules", f"protocol:tcp,ports:22,address:{cidr}",
            "--outbound-rules", "protocol:tcp,ports:all,address:0.0.0.0/0 protocol:udp,ports:all,address:0.0.0.0/0 protocol:icmp,address:0.0.0.0/0")[0]
        save("firewall.json", firewall)
    droplet = existing(api("compute", "droplet", "list"), "droplet.json")
    if not droplet:
        init = out / "cloud-init.yaml"
        init.write_text((ROOT / "deploy/digitalocean/cloud-init.yaml").read_text().replace("__SSH_PUBLIC_KEY__", json.dumps(public_key)))
        droplet = api("compute", "droplet", "create", NAME, "--region", REGION, "--size", "s-1vcpu-2gb",
            "--image", "ubuntu-24-04-x64", "--vpc-uuid", vpc["id"], "--ssh-keys", str(key["id"]),
            "--tag-name", NAME, "--user-data-file", str(init), "--enable-monitoring", "--enable-backups",
            "--backup-policy-plan", "daily")[0]
        save("droplet.json", droplet)
    db = existing(api("databases", "list"), "database.json")
    if not db:
        db = api("databases", "create", NAME, "--engine", "pg", "--version", "17", "--region", REGION,
            "--size", "db-s-1vcpu-1gb", "--num-nodes", "1", "--private-network-uuid", vpc["id"], "--tag", NAME)[0]
        save("database.json", db)
    if db["status"] == "online":
        rules = api("databases", "firewalls", "list", db["id"])
        if not any(r["type"] == "tag" and r["value"] == NAME for r in rules):
            api("databases", "firewalls", "append", db["id"], "--rule", "tag:" + NAME)
    else:
        print("Database is being created. Rerun after it is online to restrict trusted sources before importing any data.")
    print(json.dumps({"droplet_id": droplet["id"], "database_id": db["id"], "vpc_id": vpc["id"]}))


if __name__ == "__main__":
    main()
