#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
#  Gives the Release workflow what it needs to sign and notarize Asiri: secrets in the
#  repository's "release" environment, which only version tags and main can use. Run it on
#  the Mac that has your Developer ID certificate, once, and again when a credential changes.
#
#    tools/release/setup-signing.sh --api-key ~/Downloads/AuthKey_ABC123DEFG.p8 \
#        --key-id ABC123DEFG --issuer 01234567-89ab-cdef-0123-456789abcdef
#
#  The API key comes from App Store Connect › Users and Access › Integrations › App Store
#  Connect API › Team Keys (access: Developer); the issuer ID is shown above the keys. macOS
#  asks before the certificate leaves the keychain: enter your login password and choose
#  Allow. Nothing secret is printed or left on disk. The API key is also saved as the
#  notarytool profile "asiri", for releases built on this Mac (make-release.sh
#  --notary-profile asiri).
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

usage() { sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'; }
say() { printf '\033[1;35m▸\033[0m %s\n' "$*"; }
ok() { printf '\033[1;32m✓\033[0m %s\n' "$*"; }
die() { printf '\033[1;31m✗ %s\033[0m\n' "$*" >&2; exit 1; }

api_key="" key_id="" issuer=""
while [ $# -gt 0 ]; do
  case "$1" in
    --api-key|--key-id|--issuer)
      [ $# -ge 2 ] || die "$1 needs a value"
      case "$1" in
        --api-key) api_key="$2" ;;
        --key-id) key_id="$2" ;;
        --issuer) issuer="$2" ;;
      esac
      shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1 (try --help)" >&2; exit 2 ;;
  esac
done
[ -n "$api_key" ] && [ -n "$key_id" ] && [ -n "$issuer" ] || { usage >&2; exit 2; }
[ -f "$api_key" ] || die "No API key at $api_key"
[[ "$key_id" =~ ^[A-Z0-9]{10}$ ]] || die "--key-id looks like ABC123DEFG (ten letters and digits)"
[[ "$issuer" =~ ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$ ]] \
  || die "--issuer looks like 01234567-89ab-cdef-0123-456789abcdef"
[ "$(uname)" = Darwin ] || die "Run this on the Mac that has your Developer ID certificate"
command -v gh >/dev/null 2>&1 || die "The GitHub CLI is needed: brew install gh"
repo="$(gh repo view --json nameWithOwner -q .nameWithOwner)" || die "gh isn't signed in (gh auth login)"

identity="$(security find-identity -v -p codesigning \
  | sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p' | head -1)"
[ -n "$identity" ] || die "No \"Developer ID Application\" certificate in the keychain"

say "Checking the API key with Apple"
xcrun notarytool history --key "$api_key" --key-id "$key_id" --issuer "$issuer" >/dev/null 2>&1 \
  || die "Apple didn't accept the API key: check the key ID, the issuer ID and the key's access"
ok "Apple accepts it"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
password="$(openssl rand -hex 24)"

# Exports just this identity (security export would take every identity in the keychain).
cat > "$work/export.swift" <<'SWIFT'
import Foundation
import Security

let name = CommandLine.arguments[1]
let out = URL(fileURLWithPath: CommandLine.arguments[2])
let password = ProcessInfo.processInfo.environment["P12_PASSWORD"] ?? ""

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data("\(message)\n".utf8))
    exit(1)
}

let query: [String: Any] = [
    kSecClass as String: kSecClassIdentity,
    kSecMatchLimit as String: kSecMatchLimitAll,
    kSecReturnRef as String: true,
]
var items: CFTypeRef?
guard SecItemCopyMatching(query as CFDictionary, &items) == errSecSuccess,
      let identities = items as? [SecIdentity] else { fail("No identities in the keychain") }
let match = identities.first { identity in
    var certificate: SecCertificate?
    guard SecIdentityCopyCertificate(identity, &certificate) == errSecSuccess, let certificate else { return false }
    return SecCertificateCopySubjectSummary(certificate) as String? == name
}
guard let identity = match else { fail("\(name) isn't in the keychain") }

var parameters = SecItemImportExportKeyParameters()
parameters.version = UInt32(SEC_KEY_IMPORT_EXPORT_PARAMS_VERSION)
parameters.passphrase = Unmanaged.passRetained(password as CFString)
var data: CFData?
let status = SecItemExport(identity, .formatPKCS12, [], &parameters, &data)
guard status == errSecSuccess, let data else {
    fail("Couldn't export \(name): \(SecCopyErrorMessageString(status, nil) as String? ?? "error \(status)")")
}
do { try (data as Data).write(to: out, options: .withoutOverwriting) } catch { fail("\(error)") }
SWIFT

say "Exporting \"$identity\" (macOS asks for your login password: choose Allow)"
P12_PASSWORD="$password" swift "$work/export.swift" "$identity" "$work/identity.p12" \
  || die "The certificate wasn't exported"
ok "Exported it, encrypted with a one-time password"

say "Setting up the \"release\" environment in $repo"
gh api -X PUT "repos/$repo/environments/release" --input - >/dev/null <<'JSON'
{"deployment_branch_policy": {"protected_branches": false, "custom_branch_policies": true}}
JSON
policies="$(gh api "repos/$repo/environments/release/deployment-branch-policies" \
  -q '.branch_policies[] | "\(.type) \(.name)"')"
for policy in "tag v*" "branch main"; do
  grep -qxF "$policy" <<<"$policies" \
    || gh api -X POST "repos/$repo/environments/release/deployment-branch-policies" \
      -f type="${policy%% *}" -f name="${policy#* }" >/dev/null
done
ok "Only version tags and main can use its secrets"

secret() { gh secret set "$1" --env release --repo "$repo" >/dev/null; }
base64 -i "$work/identity.p12" | secret DEVELOPER_ID_CERTIFICATE
printf '%s' "$password" | secret DEVELOPER_ID_CERTIFICATE_PASSWORD
secret NOTARY_API_KEY < "$api_key"
printf '%s' "$key_id" | secret NOTARY_API_KEY_ID
printf '%s' "$issuer" | secret NOTARY_API_ISSUER
ok "Stored the certificate and the API key as the environment's secrets"

say "Saving the API key as the notarytool profile \"asiri\" on this Mac"
xcrun notarytool store-credentials asiri --key "$api_key" --key-id "$key_id" --issuer "$issuer" >/dev/null \
  && ok "Saved" || echo "(couldn't save it; releases from this Mac can pass the key in ASIRI_NOTARY_KEY instead)" >&2

cat <<EOF

The Release workflow can now sign and notarize. Keep $api_key somewhere safe (a password
manager, say) or delete it: Apple won't let you download it again, and you can always make
a new key and run this again.
EOF
