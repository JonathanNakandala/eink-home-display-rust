// The firmware's own joining code (core/join.h) over its own TLS and EST code (tls/est_client.h and the rest), run
// against a real server on a computer, with the same mbedTLS the chip is built with. Each test is a display that joins,
// renews, or is turned away, as it would on the chip but for the flash, the radio and the clock being the computer's.
//
// Run through with_fixture.sh, which starts the server and says where it is.
#include <string>

#include "host_support.h"
#include "home_display/core/profile.h"

using home_display_join::Joiner;
using home_display_join::Outcome;
using home_display_pairing::Standing;
using home_display_report::Failure;
using host::MemoryIdentity;

using namespace support;

TEST(a_new_display_waits_shows_the_code_the_server_approves_and_is_then_paired) {
  Display d("host-new");
  const Outcome waiting = d.wake();
  CHECK(waiting.standing == Standing::WAITING);
  CHECK(waiting.failure == Failure::APPROVAL);
  CHECK(!waiting.paired);
  CHECK_EQ(waiting.retry_after_s, 300u);
  CHECK_EQ(waiting.code.size(), (size_t) 14);
  CHECK(d.identity.has_key());
  CHECK(d.identity.stored.empty());  // no certificate has confirmed the root yet
  CHECK(!d.identity.has_cert);

  // The owner types the code from the panel in at the server; the server accepts it only if it worked out the same.
  CHECK_EQ(ctl("approve host-new " + waiting.code), 0);

  const Outcome paired = d.wake();
  CHECK(paired.paired);
  CHECK(paired.standing == Standing::PAIRED);
  CHECK(paired.failure == Failure::NONE);
  CHECK(!d.identity.stored.empty());  // kept now that a certificate has been issued under it
  CHECK(d.identity.has_cert);
  // The server dates it from an hour ago, to allow for a clock a little behind, and ends it 90 days from now.
  CHECK_EQ(d.identity.life.not_after - d.identity.life.not_before, 90 * DAY + 3600);
}

TEST(a_wrong_code_is_not_approved) {
  Display d("host-wrong");
  const Outcome waiting = d.wake();
  CHECK(waiting.standing == Standing::WAITING);
  std::string wrong = waiting.code;
  wrong[0] = wrong[0] == '0' ? '1' : '0';
  CHECK(ctl("approve host-wrong " + wrong) != 0);
  CHECK(d.wake().standing == Standing::WAITING);
}

TEST(a_paired_display_goes_on_without_asking_the_network_anything) {
  Display d("host-steady");
  join(d);
  d.est.roots = d.est.enrolled = d.est.renewed = 0;
  const Outcome again = d.wake();
  CHECK(again.paired);
  CHECK_EQ(d.est.requests(), 0);
}

TEST(with_a_third_of_the_certificate_left_it_renews_with_no_owner) {
  Display d("host-renew");
  join(d);
  const home_display_ports::Bytes first = d.identity.certificate_der;
  d.clock.offset = 61 * DAY;
  d.est.roots = d.est.enrolled = d.est.renewed = 0;
  const Outcome renewed = d.wake();
  CHECK(renewed.paired);
  CHECK_EQ(d.est.renewed, 1);
  CHECK_EQ(d.est.enrolled, 0);
  CHECK(d.identity.certificate_der != first);  // a new certificate
}

TEST(a_certificate_that_ended_is_asked_for_again_with_the_same_key_and_no_owner) {
  Display d("host-expired");
  join(d);
  // The certificate the display holds ended ten days ago, by its own clock. (Moving the clock instead would make the
  // new certificate, which the server dates from the real time, look over as well.)
  const int64_t now = d.clock.now();
  d.identity.life = {now - 100 * DAY, now - 10 * DAY};
  const home_display_ports::Bytes first = d.identity.certificate_der;
  d.est.roots = d.est.enrolled = d.est.renewed = 0;
  const Outcome again = d.wake();
  CHECK(again.paired);
  CHECK_EQ(d.est.enrolled, 1);
  CHECK_EQ(d.est.roots, 0);
  CHECK_EQ(d.est.renewed, 0);
  CHECK(d.identity.certificate_der != first);
  CHECK(d.identity.life.not_after > now);
}

TEST(a_server_that_does_not_chain_to_the_pinned_root_is_refused_before_anything_is_sent) {
  Display d("host-middle");
  d.identity.compiled = someone_elses_root();
  CHECK(!d.identity.compiled.empty());
  const Outcome out = d.wake();
  CHECK(out.failure == Failure::CERTIFICATE);
  CHECK(!out.paired);
  CHECK(!d.identity.has_cert);
}

TEST(a_server_that_is_not_there_is_reported_as_the_server_and_changes_nothing) {
  Display d("host-gone", 1);  // nothing listens on port 1
  const Outcome out = d.wake();
  CHECK(out.failure == Failure::SERVER);
  CHECK(!d.identity.has_key());
  CHECK(d.identity.last == home_display_pairing::Answer::NONE);
}

TEST(a_display_the_owner_has_revoked_is_not_kept_but_loses_nothing_it_holds) {
  Display d("host-revoked");
  join(d);
  CHECK_EQ(ctl("revoke host-revoked"), 0);
  d.clock.offset = 70 * DAY;  // due to renew
  const home_display_ports::Bytes key_spki = d.identity.spki();
  d.est.roots = d.est.enrolled = d.est.renewed = 0;
  const Outcome out = d.wake();
  // The server refuses the renewal, so it asks as a new display, and is refused that too: not recognised, and told so.
  CHECK_EQ(d.est.renewed, 1);
  CHECK_EQ(d.est.enrolled, 1);
  CHECK_EQ(d.est.roots, 0);
  CHECK(!out.paired);
  CHECK(out.standing == Standing::NOT_RECOGNISED);
  CHECK(out.failure == Failure::UNRECOGNISED);
  CHECK(d.identity.last == home_display_pairing::Answer::REFUSED);
  // The panel has the code to give the owner.
  CHECK_EQ(out.code.size(), (size_t) 14);
  // Pairing is removed only by the owner: the key, the certificate and the root are all still there.
  CHECK(d.identity.has_key() && d.identity.has_cert && !d.identity.stored.empty());
  CHECK(d.identity.spki() == key_spki);
}

TEST(once_the_owner_approves_it_again_a_revoked_display_is_back) {
  Display d("host-back");
  join(d);
  CHECK_EQ(ctl("revoke host-back"), 0);
  d.clock.offset = 70 * DAY;
  const Outcome refused = d.wake();
  CHECK(refused.standing == Standing::NOT_RECOGNISED);
  // It was turned away; asked again (the window is open), the owner can now approve what it shows.
  CHECK_EQ(ctl("forget host-back"), 0);
  const Outcome waiting = d.wake();
  CHECK(waiting.standing == Standing::WAITING);
  CHECK_EQ(ctl("approve host-back " + waiting.code), 0);
  CHECK(d.wake().paired);
}

TEST(a_display_that_starts_afresh_at_every_wake_joins_and_renews_from_what_flash_holds) {
  Rebooting d("host-reboot");
  const Outcome waiting = d.wake();
  CHECK(waiting.standing == Standing::WAITING);
  CHECK_EQ(ctl("approve host-reboot " + waiting.code), 0);
  // The key made at the first wake is the one used at the second: the code is the same, and the server accepted it.
  CHECK(d.wake().paired);
  const home_display_credentials::Record joined = d.held();
  CHECK(!joined.key.empty() && !joined.root.empty() && !joined.certificate.empty());
  CHECK_EQ(joined.sequence, 3u);  // the key, then the root, then the certificate

  d.roots = d.enrolled = d.renewed = 0;
  CHECK(d.wake().paired);
  CHECK_EQ(d.roots + d.enrolled + d.renewed, 0);  // an ordinary wake asks nothing

  d.clock.offset = 61 * DAY;
  CHECK(d.wake().paired);
  CHECK_EQ(d.renewed, 1);
  CHECK(d.held().certificate != joined.certificate);
  CHECK(d.held().key == joined.key);  // the same key throughout
}

TEST(a_display_that_lost_what_the_server_last_said_just_asks_again_and_is_told_again) {
  Rebooting d("host-rtc");
  const Outcome first = d.wake();
  CHECK(first.standing == Standing::WAITING);
  d.rtc = home_display_pairing::Answer::NONE;  // a power loss clears RTC memory
  const Outcome second = d.wake();
  CHECK(second.standing == Standing::WAITING);
  CHECK_EQ(second.code, first.code);  // the same key, so the same code
}

TEST(a_power_cut_while_the_certificate_is_written_leaves_the_display_as_it_was) {
  Rebooting d("host-cut");
  const Outcome waiting = d.wake();
  CHECK_EQ(ctl("approve host-cut " + waiting.code), 0);
  CHECK(d.wake().paired);
  const home_display_credentials::Record before = d.held();

  d.clock.offset = 61 * DAY;  // due to renew
  d.flash.cut_write = 1;      // the next write, the new certificate, is cut short and the power goes
  const Outcome cut = d.wake();
  CHECK(!cut.paired);
  CHECK(cut.failure == Failure::MEMORY);

  // Power back: what it reads is the record before, whole, and it goes on from there.
  d.flash.restore_power();
  const home_display_credentials::Record after = d.held();
  CHECK_EQ(after.sequence, before.sequence);
  CHECK(after.certificate == before.certificate && after.key == before.key && after.root == before.root);
  CHECK(d.wake().paired);
  CHECK(d.held().certificate != before.certificate);
}

TEST(flash_that_cannot_be_read_is_left_alone_and_the_server_is_not_asked) {
  Rebooting d("host-broken");
  const Outcome waiting = d.wake();
  CHECK_EQ(ctl("approve host-broken " + waiting.code), 0);
  CHECK(d.wake().paired);
  const home_display_credentials::Record before = d.held();
  const int writes = d.flash.writes;

  d.flash.broken_reads["cred_a"] = true;
  d.flash.broken_reads["cred_b"] = true;
  d.roots = d.enrolled = d.renewed = 0;
  const Outcome out = d.wake();
  CHECK(!out.paired);
  CHECK(out.failure == Failure::MEMORY);
  CHECK_EQ(d.roots + d.enrolled + d.renewed, 0);
  CHECK_EQ(d.flash.writes, writes);  // nothing written over what could not be read

  d.flash.broken_reads.clear();  // the flash comes back
  CHECK(d.wake().paired);
  CHECK(d.held().key == before.key);
}

TEST(a_display_whose_pairing_is_erased_asks_again_with_a_new_key_and_the_owner_approves_it_by_the_new_code) {
  // What holding the button for ten seconds does (core/hold.h): the record is erased, whatever else is in the flash.
  Rebooting d("host-erase");
  const Outcome waiting = d.wake();
  CHECK_EQ(ctl("approve host-erase " + waiting.code), 0);
  CHECK(d.wake().paired);
  const home_display_credentials::Record before = d.held();
  CHECK(!before.key.empty());

  {
    home_display_credentials::Credentials credentials(d.flash);
    CHECK(credentials.erase());
    d.rtc = home_display_pairing::Answer::NONE;  // the chip clears what RTC memory kept of the server's last answer too
  }
  CHECK(d.held().empty());

  // It starts as a new display: fetches the root, makes a key, and asks. The name belongs to a member with another
  // key, so the server holds the request for the owner, who has to approve it with the code on the panel.
  const Outcome again = d.wake();
  CHECK(again.standing == Standing::WAITING);
  CHECK(again.code != waiting.code);  // another key, another code
  CHECK_EQ(d.roots, 3);  // fetched at each wake until a certificate is issued under it: twice before, and again

  // The old code is the old key's: not accepted. The new one is.
  CHECK(ctl("approve host-erase " + waiting.code) != 0);
  CHECK_EQ(ctl("approve host-erase " + again.code), 0);
  CHECK(d.wake().paired);
  CHECK(d.held().key != before.key);
  CHECK(d.held().certificate != before.certificate);
}

TEST(a_display_says_what_it_is_the_owner_sees_it_before_approving_and_a_newer_firmware_updates_it) {
  using home_display_profile::Profile;
  Rebooting d("host-profile");
  d.profile = home_display_profile::encode(Profile{"reTerminal E1003", "0.2.0", 1872, 1404, 16, {"bmp", "png", "qoi"}});
  CHECK(!d.profile.empty());

  // Asking: the owner can see which display it is before typing its code.
  const Outcome waiting = d.wake();
  CHECK(waiting.standing == Standing::WAITING);
  const std::string asked = ctl_output("displays list");
  CHECK(asked.find("host-profile") != std::string::npos);
  CHECK(asked.find("reTerminal E1003, 1872x1404, 16 greys, firmware 0.2.0") != std::string::npos);

  // Approved and a member, it is still said.
  CHECK_EQ(ctl("approve host-profile " + waiting.code), 0);
  CHECK(d.wake().paired);
  CHECK(ctl_output("displays list").find("firmware 0.2.0") != std::string::npos);

  // A newer firmware says so when it renews its certificate.
  d.profile = home_display_profile::encode(Profile{"reTerminal E1003", "0.3.0", 1872, 1404, 16, {"bmp", "png", "qoi"}});
  d.clock.offset = 61 * DAY;
  CHECK(d.wake().paired);
  CHECK_EQ(d.renewed, 1);
  const std::string renewed = ctl_output("displays list");
  CHECK(renewed.find("firmware 0.3.0") != std::string::npos);
  CHECK(renewed.find("firmware 0.2.0") == std::string::npos);
}

TEST(a_display_that_says_nothing_is_just_as_welcome) {
  Rebooting d("host-noprofile");
  const Outcome waiting = d.wake();
  CHECK(waiting.standing == Standing::WAITING);
  CHECK_EQ(ctl("approve host-noprofile " + waiting.code), 0);
  CHECK(d.wake().paired);
  const std::string listed = ctl_output("displays list");
  CHECK(listed.find("host-noprofile") != std::string::npos);
  CHECK(listed.find("greys") == std::string::npos);
}
