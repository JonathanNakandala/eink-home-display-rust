// The names and numbers the display and the server must agree on, in one place.
//
// The server has the same ones (src/adapters/est_server/mod.rs, src/domain/models/pairing.rs). They are written down
// once in testdata/contract/constants.vectors, and the tests on both sides check theirs against it, so one changed
// alone fails a test and not a display in the field.
#pragma once

#include <cstddef>

namespace home_display_wire {

// The name the server's certificate always has, and the only one a display checks, whatever address mDNS found.
constexpr const char *SERVER_NAME = "home-display.internal";

// RFC 9266: the label, and no context, whose exported value identifies one TLS 1.3 connection, and its length in bytes.
constexpr const char *BINDING_LABEL = "EXPORTER-Channel-Binding";
constexpr size_t BINDING_LENGTH = 32;

// The private OID of the extension in which a display says what it is, in a certificate request, in the dotted form
// (the bytes are core/der.h's `oid_profile`).
constexpr const char *PROFILE_OID = "2.25.110978727574289354506863863824604692215";

}  // namespace home_display_wire
