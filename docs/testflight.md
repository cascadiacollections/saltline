# Personal TestFlight testing

Bundle ID: `com.cascadiacollections.saltline`. Apple team: `9L45FNA7XM`.

App Store Connect record: [Saltline Dev](https://appstoreconnect.apple.com/apps/6826944922/distribution) (Apple ID `6826944922`). The **Personal Testing** internal group has automatic distribution enabled and includes the account-holder Apple Account.

Use a paid Apple Developer Program team, Xcode signed into that account in Settings > Accounts, Rust via rustup, and Godot **4.7.2** with matching iOS export templates. Set `GODOT_BIN` if your default Godot differs. Set `IOS_TEAM` to override the preset team.

The app record and internal group are already registered. For another Apple team, create an iOS app with the matching bundle ID and an internal group containing your App Store Connect user, with automatic distribution enabled. Complete any Apple agreements yourself and answer export compliance based on the app's actual use of encryption.

```sh
just testflight --check
just testflight
just testflight --upload
```

The default command creates a signed release archive under `build/testflight/<UTC build number>/`. `--upload` additionally sends it to App Store Connect with `testFlightInternalTestingOnly=true`, so it is limited to internal testers. Build numbers use UTC days since 2020, hour, and minute, fitting Apple’s four/two/two digit limits. Allow a minute between builds or use `--build-number` to provide a higher value. After Apple processes the upload, install through TestFlight using the Apple Account in your internal group. Builds expire after 90 days.

Xcode manages signing and provisioning through its existing account. No credentials or signing keys are committed. These commands do not publish an App Store release. For quick device development, open the exported Xcode project and run on your connected iPhone.

References: [Apple TestFlight](https://developer.apple.com/testflight/), [add internal testers](https://developer.apple.com/help/app-store-connect/test-a-beta-version/add-internal-testers).

Initial upload verified on October 25, 2026: build `2489.23.7`. Apple accepted the signed archive for internal TestFlight processing. Upload reported a non-blocking missing dSYM warning for the Rust GDExtension, so Rust crash reports may lack symbol names. Device installation has not yet been verified.

## Public beta

Upload with `just testflight --upload --external` to allow external testing. The default upload remains internal-only. After Apple processing, add the build to the **Public Beta** external group and submit it for Beta App Review. External testers can install after approval.

Public invitation: https://testflight.apple.com/join/PgjfndD9
