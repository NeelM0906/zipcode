"""Operator CLI for ZIPCODE GitHub invitations."""

from __future__ import annotations

import argparse
import datetime as dt

from auth import InvitationStore


def main() -> None:
    parser = argparse.ArgumentParser(description="Manage ZIPCODE invitations")
    subcommands = parser.add_subparsers(dest="command", required=True)

    invite = subcommands.add_parser("invite", help="invite a GitHub login")
    invite.add_argument("github_login")
    invite.add_argument("--days", type=int, help="expire the invitation after N days")

    revoke = subcommands.add_parser("revoke", help="revoke a GitHub login")
    revoke.add_argument("github_login")
    subcommands.add_parser("list", help="list invitations")

    args = parser.parse_args()
    store = InvitationStore()
    if args.command == "invite":
        expires_at = None
        if args.days is not None:
            expires_at = int((dt.datetime.now(dt.UTC) + dt.timedelta(days=args.days)).timestamp())
        store.invite(args.github_login, expires_at)
        print(f"Invited @{args.github_login.lower()}")
    elif args.command == "revoke":
        store.revoke(args.github_login)
        print(f"Revoked @{args.github_login.lower()}")
    else:
        for row in store.list_invitations():
            state = "enabled" if row["enabled"] else "revoked"
            expiry = row["expires_at"] or "never"
            print(f"{row['github_login']}\t{state}\texpires={expiry}")


if __name__ == "__main__":
    main()
