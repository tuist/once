def package_dep($name): .deps[] | select(endswith("_" + $name));
. as $graph
| (.NetNewsWire | package_dep("Account")) as $mac
| (."NetNewsWire-iOS" | package_dep("Account")) as $phone
| (.NetNewsWire.deps[] | select(startswith("XCFramework_"))) as $mac_binary
| (."NetNewsWire-iOS".deps[] | select(startswith("XCFramework_"))) as $phone_binary
| $mac != $phone
and $mac_binary != $phone_binary
and $graph[$mac_binary].attrs.platform == "macos"
and $graph[$phone_binary].attrs.platform == "ios"
and $graph[$mac_binary].attrs.bundle == $graph[$phone_binary].attrs.bundle
and $graph[$mac].attrs.platform == "macos"
and $graph[$phone].attrs.platform == "ios"
and $graph[$phone].attrs.minimum_os == "17.0"
and .NetNewsWire.attrs.minimum_os == "13.0"
and .NetNewsWire.srcs == ["Sources/main.swift"]
and .NetNewsWire.attrs.private_headers == ["Sources/Reader.h"]
and (.NetNewsWire.attrs | has("exported_headers") | not)
and ."NetNewsWire-iOS".srcs == ["Sources/main.swift"]
and (.NetNewsWire.attrs.swift_flags | index("GRAPH_COMMON") != null and index("GRAPH_DEBUG") != null and index("GRAPH_MAC") != null)
and (."NetNewsWire-iOS".attrs.swift_flags | index("GRAPH_IOS") != null and index("GRAPH_MAC") == null)
and .NetNewsWire.attrs.bundle_id == "dev.once.NetNewsWire"
and (.NetNewsWire.deps | index("NetNewsWire_Share_Extension") != null and index("Subscribe_to_Feed") != null)
and (."NetNewsWire-iOS".deps | index("NetNewsWire_iOS_Share_Extension") != null and index("NetNewsWire_iOS_Widget_Extension") != null)
and (.NetNewsWireTests.deps | index("NetNewsWire") != null)
and (."NetNewsWire-iOSTests".deps | index("NetNewsWire-iOS") != null)
and (.xcode.attrs._default_test_roots | sort) == (["NetNewsWireTests", "NetNewsWire-iOSTests"] | sort)
and (all(to_entries[] | select(.key | startswith("XcodePackage_")) | select(.key | endswith("_MacroHost") | not); . as $node | all(.value.deps[]; $graph[.].attrs.platform == $node.value.attrs.platform)))
and (all([$mac, $phone][]; $graph[.] as $account | ($account | package_dep("RSCore")) as $core | ($account | package_dep("RSParser")) as $parser | $graph[$parser].deps == [$core]))
