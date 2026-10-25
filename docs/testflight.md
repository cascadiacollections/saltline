# Personal TestFlight testing

Bundle ID: `com.cascadiacollections.saltline`.

Use a paid Apple Developer Program team, Xcode signed into that account in Settings > Accounts, Rust via rustup, and Godot **4.7.2** with matching iOS export templates. Set `GODOT_BIN` if your default Godot differs. Set `IOS_TEAM` to override the preset team.

In App Store Connect, create an iOS app with this bundle ID, then create an internal testing group containing your App Store Connect user. Enable automatic distribution for the group. Complete any Apple agreements yourself and answer export compliance based on the app's actual use of encryption.

```sh
just testflight --check
just testflight
just testflight --upload
```

The default command creates a signed release archive under `build/testflight/<UTC build number>/`. `--upload` additionally sends it to App Store Connect with `testFlightInternalTestingOnly=true`, so it is limited to internal testers. Build numbers use UTC days since 2020, hour, and minute, fitting Apple’s four/two/two digit limits. Allow a minute between builds or use `--build-number` to provide a higher value. After Apple processes the upload, install through TestFlight using the Apple Account in your internal group. Builds expire after 90 days.

Xcode manages signing and provisioning through its existing account. No credentials or signing keys are committed. These commands do not publish an App Store release. For quick device development, open the exported Xcode project and run on your connected iPhone.

References: [Apple TestFlight](https://developer.apple.com/testflight/), [add internal testers](https://developer.apple.com/help/app-store-connect/test-a-beta-version/add-internal-testers).
