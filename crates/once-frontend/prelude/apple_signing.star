_APPLE_IDENTITY_FILE_EXTENSIONS = {".p12": "p12", ".pfx": "p12", ".pem": "pem"}
_APPLE_PROFILE_PLATFORMS = {"ios": ["iOS"], "macos": ["OSX"], "macosx": ["OSX"], "tvos": ["tvOS"], "visionos": ["xrOS", "visionOS"]}

def _apple_host_absolute(ctx, path):
    resolved = _package_relative(ctx, path)
    return resolved if resolved.startswith("/") else workspace_root() + "/" + resolved

def _apple_signing(ctx, attrs, platform, sdk_variant):
    """Resolve the signing mode. Returns None for ad hoc signing.

    A development identity is either a key file (PKCS #12 or PEM holding
    the certificate and private key), signed with rcodesign on any host, or
    a keychain identity selector passed to macOS codesign. Key material is
    never declared as an action input or output and never appears in argv
    or env; actions reference the file by path and partition their cache
    identity by its digest.
    """
    label = ctx["label"]["id"]
    mode = attrs.get("signing") or "ad_hoc"
    identity = attrs.get("signing_identity") or ""
    profile = attrs.get("provisioning_profile") or ""
    password_file = attrs.get("signing_identity_password_file") or ""
    if mode == "ad_hoc":
        if identity or profile or password_file:
            fail(label + ": signing_identity, signing_identity_password_file and provisioning_profile require signing = \"development\"")
        return None
    if mode != "development":
        fail(label + ": attribute `signing` must be `ad_hoc` or `development`, got `" + mode + "`")
    if platform not in ["macos", "macosx"] and sdk_variant != "device":
        fail(label + ": development signing produces device builds; set sdk_variant = \"device\", or use signing = \"ad_hoc\" for the simulator")
    if not identity or not profile:
        fail(label + ": signing = \"development\" requires signing_identity (a .p12, .pfx or .pem file, or a macOS keychain identity) and provisioning_profile (a .mobileprovision or .provisionprofile file)")
    profile_path = _apple_host_absolute(ctx, profile)
    if not host_file_exists(profile_path):
        fail(label + ": provisioning profile `" + profile + "` does not exist; download a development profile for this bundle_id and set provisioning_profile to its path")
    identity_format = ""
    for extension, kind in _APPLE_IDENTITY_FILE_EXTENSIONS.items():
        if identity.lower().endswith(extension):
            identity_format = kind
    if not identity_format and "/" in identity:
        identity_format = "pem"
    signing = {"mode": mode, "profile": profile_path, "keychain_identity": ""}
    if identity_format:
        identity_path = _apple_host_absolute(ctx, identity)
        if not host_file_exists(identity_path):
            fail(label + ": signing identity file `" + identity + "` does not exist")
        signing["identity_file"] = identity_path
        signing["identity_format"] = identity_format
        signing["identity_digest"] = host_file_sha256(identity_path)
        signing["password_file"] = ""
        if identity_format == "p12" and not password_file:
            fail(label + ": a .p12 or .pfx signing_identity requires signing_identity_password_file; for a key exported without a password, point it at an empty file")
        if password_file:
            if identity_format != "p12":
                fail(label + ": signing_identity_password_file only applies to .p12 or .pfx identities")
            signing["password_file"] = _apple_host_absolute(ctx, password_file)
            if not host_file_exists(signing["password_file"]):
                fail(label + ": signing identity password file `" + password_file + "` does not exist")
    else:
        if password_file:
            fail(label + ": signing_identity_password_file only applies to .p12 or .pfx identities")
        signing["keychain_identity"] = identity
    return signing

def _apple_codesign_with_signing(codesign, signing, label):
    """Bind a resolved codesign tool to a development identity."""
    if not signing:
        return codesign
    if _apple_execution_toolchain():
        fail(label + ": development signing reads the identity from the analysis host and is not available with Apple workers; build with signing = \"ad_hoc\" there and sign on a host that holds the identity")
    if signing["keychain_identity"]:
        if codesign.get("backend") == "rcodesign":
            fail(label + ": keychain signing identities need macOS codesign; on this host pass a .p12 or .pem file as signing_identity")
        fingerprint = _apple_keychain_fingerprint(signing["keychain_identity"], label)
        out = dict(codesign)
        out["sign_identity"] = fingerprint
        out["identity"] = codesign["identity"] + "\x00keychain\x00" + fingerprint
        return out
    out = dict(codesign)
    if codesign.get("backend") != "rcodesign":
        path = host_which_optional("rcodesign")
        if not path:
            fail(label + ": file signing identities sign with rcodesign; install rcodesign, or import the identity into a keychain and set signing_identity to its name or SHA-1 fingerprint")
        out = {"codesign_path": path, "backend": "rcodesign", "identity": "once.apple.rcodesign.v1\x00" + path + "\x00" + host_file_sha256(path), "env": {"PATH": "/usr/bin:/bin"}}
    if signing["identity_format"] == "p12":
        args = ["--p12-file", signing["identity_file"]]
        if signing["password_file"]:
            args.extend(["--p12-password-file", signing["password_file"]])
    else:
        args = ["--pem-file", signing["identity_file"]]
    out["sign_args"] = args
    out["identity"] = out["identity"] + "\x00file\x00" + signing["identity_digest"]
    return out

def _apple_keychain_fingerprint(selector, label):
    """Resolve a keychain identity name or SHA-1 to the certificate's SHA-1.

    Signing and cache identity use the fingerprint, so a renewed certificate
    with the same name re-signs instead of replaying the old signature.
    """
    output = host_command([host_which("security"), "find-identity", "-v", "-p", "codesigning"])
    identities = []
    for line in output.split("\n"):
        parts = line.strip().split(" ", 2)
        if len(parts) == 3 and parts[0].endswith(")") and len(parts[1]) == 40:
            identities.append((parts[1].upper(), parts[2].strip('"')))
    matches = [fingerprint for fingerprint, name in identities if fingerprint == selector.upper() or name == selector]
    if len(matches) == 1:
        return matches[0]
    names = ", ".join(["`" + name + "` (" + fingerprint + ")" for fingerprint, name in identities]) or "none"
    if matches:
        fail(label + ": several keychain identities are named `" + selector + "`; set signing_identity to one SHA-1 fingerprint: " + names)
    fail(label + ": no valid code signing identity `" + selector + "` in the keychain; set signing_identity to one of " + names + ", or to a .p12 or .pem file")

def _apple_codesign_argv(codesign, path, entitlements = ""):
    if codesign.get("backend") == "rcodesign":
        argv = [codesign["codesign_path"], "sign", "--shallow", "--timestamp-url", "none"] + (codesign.get("sign_args") or [])
        if entitlements:
            argv.extend(["--entitlements-xml-file", entitlements])
    else:
        argv = [codesign["codesign_path"], "--force", "--sign", codesign.get("sign_identity") or "-", "--timestamp=none"]
        if entitlements:
            argv.extend(["--entitlements", entitlements])
    return argv + [path]

def _apple_entitlement_pattern_matches(pattern, value):
    if type(pattern) != "string" or type(value) != "string":
        return False
    if pattern.endswith("*"):
        return value.startswith(pattern[:-1])
    return pattern == value

def _apple_entitlement_allowed(value, allowed):
    if type(value) == "bool":
        return value == False or allowed == True
    if type(value) == "string":
        patterns = allowed if type(allowed) == "list" else [allowed]
        return any([_apple_entitlement_pattern_matches(pattern, value) for pattern in patterns])
    if type(value) == "list":
        patterns = allowed if type(allowed) == "list" else [allowed]
        for item in value:
            if not any([_apple_entitlement_pattern_matches(pattern, item) for pattern in patterns]):
                return False
        return True
    return value == allowed

def _apple_read_profile(ctx, signing):
    label = ctx["label"]["id"]
    profile = plist_decode(host_file_cms_content(signing["profile"]))
    if type(profile) != "dict" or type(profile.get("Entitlements") or {}) != "dict" or type(profile.get("TeamIdentifier") or []) != "list" or type(profile.get("Platform") or []) != "list":
        fail(label + ": provisioning profile `" + signing["profile"] + "` is malformed; download it again from the developer portal")
    teams = profile.get("TeamIdentifier") or []
    team = teams[0] if teams else (profile.get("Entitlements") or {}).get("com.apple.developer.team-identifier", "")
    if type(team) != "string" or not team:
        fail(label + ": provisioning profile `" + signing["profile"] + "` has no TeamIdentifier")
    return team, profile

def _apple_development_entitlements(ctx, signing, platform, bundle_id, development_team, content):
    """Derive signing entitlements from the profile and validate requested ones.

    The profile fixes `application-identifier` and the team identifier, and
    supplies `get-task-allow` unless `content` lowers it. Every entitlement in
    `content` must be granted by the profile; string values may use the
    profile's trailing `*` wildcards. macOS sandbox and hardened runtime
    entitlements (`com.apple.security.*`) are not provisioned, so on macOS
    those absent from the profile pass through.
    """
    label = ctx["label"]["id"]
    team, profile = _apple_read_profile(ctx, signing)
    if development_team and development_team != team:
        fail(label + ": development_team `" + development_team + "` does not match provisioning profile team `" + team + "`")
    names = _APPLE_PROFILE_PLATFORMS.get(platform)
    platforms = profile.get("Platform") or []
    if names and platforms and not any([name in platforms for name in names]):
        fail(label + ": provisioning profile `" + signing["profile"] + "` is for " + ", ".join(platforms) + ", not " + platform)
    granted = profile.get("Entitlements") or {}
    application_identifier = team + "." + bundle_id
    allowed_identifier = granted.get("application-identifier") or granted.get("com.apple.application-identifier") or ""
    if not _apple_entitlement_pattern_matches(allowed_identifier, application_identifier):
        fail(label + ": provisioning profile `" + signing["profile"] + "` allows application identifier `" + allowed_identifier + "`, not `" + application_identifier + "`; use a profile for bundle_id `" + bundle_id + "`")
    is_macos = platform in ["macos", "macosx"]
    identifier_key = "com.apple.application-identifier" if is_macos else "application-identifier"
    derived = {identifier_key: application_identifier, "com.apple.developer.team-identifier": team}
    for key in ["get-task-allow", "com.apple.security.get-task-allow"]:
        if key in granted:
            derived[key] = granted[key]
    requested = {}
    if content:
        content = content.replace("$(AppIdentifierPrefix)", team + ".").replace("${AppIdentifierPrefix}", team + ".")
        content = content.replace("$(TeamIdentifierPrefix)", team + ".").replace("${TeamIdentifierPrefix}", team + ".")
        requested = plist_decode(content)
        if type(requested) != "dict":
            fail(label + ": entitlements must be a property list dictionary")
    for key, value in requested.items():
        if key in [identifier_key, "com.apple.developer.team-identifier"] and value != derived[key]:
            fail(label + ": entitlement `" + key + "` is `" + str(value) + "`, but provisioning profile `" + signing["profile"] + "` requires `" + str(derived[key]) + "`; remove it from entitlements")
        if key not in granted and is_macos and key.startswith("com.apple.security."):
            derived[key] = value
            continue
        if key not in granted:
            fail(label + ": entitlement `" + key + "` is not granted by provisioning profile `" + signing["profile"] + "`; enable the capability for the App ID and regenerate the profile, or remove it from entitlements")
        if not _apple_entitlement_allowed(value, granted[key]):
            fail(label + ": entitlement `" + key + "` value `" + str(value) + "` is outside what provisioning profile `" + signing["profile"] + "` grants (`" + str(granted[key]) + "`)")
        derived[key] = value
    return plist_encode(derived)

def _apple_embed_provisioning_profile(ctx, signing, contents_dir, platform):
    name = "embedded.provisionprofile" if platform in ["macos", "macosx"] else "embedded.mobileprovision"
    destination = declare_output(contents_dir + "/" + name)
    materialize_host_file(
        signing["profile"],
        destination,
        display_name = "Embed provisioning profile · " + ctx["label"]["name"],
    )
    return destination
