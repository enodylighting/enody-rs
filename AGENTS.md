# Enody Rust SDK Guide

This repository is the public `enody` Rust SDK for discovering, configuring,
and controlling Enody lighting products from host applications. Treat this file
as a quick orientation for third-party developers building against the SDK.

The crate supports two main uses:

- Host-side applications with the default `remote` and `cli` features.
- Embedded or constrained integrations with `default-features = false`, where
  the shared message types and local traits remain available without `std`.

## Repository Layout

- `src/lib.rs`: crate modules, feature gates, shared `Identifier` and `Error`
  types.
- `src/message.rs`: public command/event protocol, topology info structs,
  configuration types, WiFi network types, settings, and tokens.
- `src/runtime.rs`: local `Runtime` trait and the `remote::RemoteRuntime`
  command dispatcher.
- `src/environment.rs`: discovery traits for remote environments.
- `src/usb/`: USB discovery and transport backends.
- `src/wifi.rs`: WiFi wire types, mDNS discovery, authenticated TCP transport,
  and WiFi token pairing.
- `src/host.rs`, `src/fixture.rs`, `src/source.rs`, `src/emitter.rs`: public
  topology traits and remote handle APIs.
- `src/spectral.rs`: spectral sample and downloaded spectral data structures.
- `src/token_store.rs`: CLI token persistence.
- `src/main.rs`: the `enody` command-line tool and examples of intended SDK
  usage.

## Feature Flags

- `remote` enables host-side discovery and transport APIs. It implies `std`.
- `cli` enables the `enody` binary.
- `std` enables standard library support.

Default features are `remote` and `cli`. Disable default features for a no_std
consumer that only needs the shared traits, messages, serialization, spectral
types, and WiFi wire definitions.

## Product Interface Model

The SDK models Enody products as a hierarchy:

```text
Environment -> RemoteRuntime -> RemoteHost -> RemoteFixture -> RemoteSource -> RemoteEmitter
```

The levels mean:

- `Environment`: a discovery surface such as USB or WiFi.
- `RemoteRuntime`: one active connection to one product runtime.
- `RemoteHost`: the product host returned by a runtime.
- `RemoteFixture`: an addressable light output unit.
- `RemoteSource`: an independently controllable region within a fixture.
- `RemoteEmitter`: an individual LED/emitter channel.

All product resources are identified by `Identifier`, which is a `uuid::Uuid`.

The local traits are synchronous and are useful when implementing compatible
local or embedded components:

- `host::Host`: `identifier()`, `version()`, `fixtures()`
- `fixture::Fixture`: `identifier()`, `display(config, flux)`, `sources()`
- `source::Source`: `identifier()`, `display(config, flux)`, `emitters()`
- `emitter::Emitter`: `identifier()`, `flux_range()`, `set_flux()`,
  `spectral_data()`

The host-side remote APIs are async inherent methods on concrete remote types.
Remote objects own a cloned `runtime::remote::RemoteRuntime`; cloning is cheap
and shares the same underlying connection, so callers can pass remote hosts,
fixtures, sources, and emitters around without parent lifetimes.

## Discovery And Traversal

Remote traversal is count-then-index based:

- `RemoteRuntime::host()` queries `HostCommand::Info`.
- `RemoteHost::fixtures()` queries `FixtureCount`, then `FixtureInfo(index)`.
- `RemoteFixture::sources()` queries `SourceCount`, then `SourceInfo(index)`.
- `RemoteSource::emitters()` queries `EmitterCount`, then
  `EmitterInfo(index)`.

Typical USB discovery:

```rust
use enody::{
    environment::Environment,
    usb::UsbEnvironment,
};

let environment = UsbEnvironment::new();
for runtime in environment.runtimes() {
    let host = runtime.host().await?;
    let fixtures = host.fixtures().await?;
    for fixture in fixtures {
        let sources = fixture.sources().await?;
        for source in sources {
            let emitters = source.emitters().await?;
            println!("source {} has {} emitters", source.identifier(), emitters.len());
        }
    }
}
```

For hotplug or continuous discovery, use `environment::DiscoveryEnvironment`.
`UsbEnvironment` and `WifiEnvironment` both emit
`EnvironmentRuntimeEvent::Arrived` and `EnvironmentRuntimeEvent::Left`.

## Light Control

Use `message::Configuration` and `message::Flux` for display commands:

```rust
use enody::message::{Chromaticity, Configuration, Flux};

fixture
    .display(Configuration::Blackbody(4000.0), Flux::Relative(0.5))
    .await?;

source
    .display(
        Configuration::Chromatic(Chromaticity { x: 0.3127, y: 0.3290 }),
        Flux::Relative(1.0),
    )
    .await?;
```

Emitter-level control is explicit:

```rust
emitter.set_flux(Flux::Relative(0.8)).await?;
fixture
    .display(Configuration::Manual, Flux::Relative(1.0))
    .await?;
```

`RemoteEmitter::set_flux()` updates a per-emitter target. A later
`Configuration::Manual` display applies those per-emitter values rather than
asking the product to compute a blackbody or chromatic mix.

`RemoteEmitter::spectral_data()` downloads spectral data by querying sample
count and reading samples in `SampleBatch` chunks.

Long-running fixture/source animation uses `message::Transition<State>`:

```rust
use core::time::Duration;
use enody::message::{
    Configuration, FixtureState, Flux, SourceState, Transition, TransitionMethod,
};

fixture
    .transition(Transition {
        target: FixtureState::new(Configuration::Blackbody(2700.0), Flux::Relative(0.4)),
        method: TransitionMethod::Linear(Duration::from_secs(2)),
    })
    .await?;

source
    .transition(Transition {
        target: SourceState::new(Configuration::Flux, Flux::Relative(0.0)),
        method: TransitionMethod::Linear(Duration::from_millis(750)),
    })
    .await?;
```

The runtime emits context-matched `TransitionStart(current_state, transition)`,
`TransitionProgress(transition, state, progress)`, and
`TransitionEnd(transition, final_state)` events. The remote helper methods use
`execute_command_with_timeout_until()` and return after the end event. If a
gesture or another command interrupts the transition, `final_state` is the state
at interruption time rather than the transition target. A new transition for the
same receiver takes over from that interrupted state, while non-transition
commands should continue receiving responses during the animation. Do not add
emitter transitions; emitter control remains explicit flux setting plus
source/fixture display.

## Runtime And Message Behavior

All remote transports carry `message::Message` values:

```rust
Message::Command(CommandMessage)
Message::Event(EventMessage)
```

`CommandMessage::root(command, resource)` creates a command with a new UUID and
an optional target resource UUID. A response event uses the command UUID as its
`context`, and `RemoteRuntime` uses that context to match responses to pending
commands.

`runtime::remote::RemoteRuntimeConnection` is the transport abstraction. It only
connects, disconnects, sends messages, and receives messages. `RemoteRuntime`
adds the higher-level behavior:

- `connect()` starts the background dispatch task.
- `execute_command()` sends one command and waits for a matching response.
- `execute_command_with_timeout_until()` handles multi-event operations such as
  WiFi scans, joins, token generation, and transitions. It consumes
  context-matched intermediate events until the terminal predicate is satisfied.
- `send_command()` and `send_event()` are fire-and-forget helpers.
- `next_message()` returns unmatched messages.
- `enable_logging()` consumes `RuntimeEvent::Log` messages and forwards them to
  the `log` crate.

USB messages are postcard-serialized and framed with STX/ETX plus DLE escaping.
The shared encoder and streaming parser live in `src/serialization.rs`.

## WiFi Discovery

WiFi discovery uses mDNS for `_enody._tcp.local`. Discovered devices are
represented as `wifi::WifiDiscoveredDevice`.

Relevant public constants in `src/wifi.rs`:

- TCP API port: `WIFI_PORT` (`8788`)
- API version: `WIFI_API_VERSION` (`1`)
- Protocol TXT value: `WIFI_PROTOCOL` (`enody-v1`)
- Auth TXT value: `WIFI_AUTH` (`noise-psk`)
- Authenticated Noise pattern: `WIFI_NOISE`
- Pairing Noise pattern: `WIFI_PAIRING_NOISE`

`WifiEnvironment::new(tokens)` and its timeout/exclusion variants discover
devices over mDNS, keep devices that match a saved `Token`, and connect them as
`RemoteRuntime`s through `WifiConnection`. Excluded host IDs are useful when an
application has already found the same product over USB and wants to avoid
showing duplicates.

`WifiEnvironment::start_discovery()` polls mDNS in the background and emits
arrival/left events. A connected WiFi runtime is marked left after repeated
missed discovery polls.

## WiFi Authentication

Normal WiFi control requires a saved `message::Token`. A token contains:

- `host_id`: the product host UUID.
- `key_id`: the token identifier used in `Request::Hello`.
- `data`: the shared key material.

`WifiConnection` opens TCP, sends `Request::Hello { version, key_id }`, and then
performs a PSK Noise handshake using the token data. After authentication,
regular SDK `Message` values are postcard-serialized, encrypted, and exchanged
inside `Request::Noise` and `Response::Noise` frames.

Use `WifiConnection::runtime_from_endpoint(token, endpoint)` when the endpoint
is known, or `WifiConnection::runtime_from_discovered_device(token, device)`
after mDNS discovery.

## WiFi Network Setup

WiFi network setup means giving a product an SSID and credentials. The CLI flow
is `enody wifi-setup`, and it uses a trusted USB runtime:

1. `host.wifi_scan()` scans nearby WiFi networks.
2. The user chooses or types an SSID and enters a password.
3. `host.wifi_join(ssid, password)` sends the network credentials to the
   product.
4. The CLI calls `RemoteRuntime::generate_token()` over the same trusted USB
   runtime.
5. The token is saved locally for future WiFi connections.

For application code, the lower-level methods are:

- `RemoteHost::wifi_scan()`
- `RemoteHost::wifi_join(ssid, password)`
- `RemoteHost::network_scan(filters)`
- `RemoteHost::network_join(network, credentials)`
- `RemoteRuntime::generate_token()`

Host-side CLI tokens are stored in `tokens.json` under the first available base
path in this order: `XDG_CONFIG_HOME/enody`, `~/.enody`,
`%USERPROFILE%/.enody`, then `%APPDATA%/enody`. Tokens are upserted by
`host_id`.

## WiFi Token Pairing

WiFi token pairing means generating a new authentication token over the network
after physical user approval. It does not configure the product onto a WiFi
network; the product must already be reachable over WiFi.

The CLI flow is `enody wifi-generate-token`:

1. Discover pairing candidates with
   `WifiConnection::discover_token_generation_devices(timeout)`.
2. Choose a `WifiDiscoveredDevice` and resolve its endpoint.
3. Call
   `WifiConnection::generate_token_from_discovered_device_with_approval(...)`.
4. Show each approval instruction received by the callback to the user.
5. When the product approves, the SDK receives `RuntimeEvent::TokenGenerated`.
6. The CLI verifies the token by opening a normal authenticated WiFi runtime and
   calling `runtime.host()`.
7. The token is saved locally with `token_store::TokenStore`.

At the protocol level, pairing uses an unauthenticated Noise session:

1. The client sends `Request::PairingNoise { version, payload }`.
2. The pairing Noise prologue is `"enody-v1 pairing"`.
3. The product responds with `Response::PairingNoise`.
4. The SDK reads encrypted pairing messages until it receives either an
   approval instruction, a generated token, an error, or a timeout.

Use the `*_with_approval` APIs so your app can display the product-provided
physical approval instruction. Do not hard-code a gesture or approval text in a
third-party app; the instruction comes from the product.

## CLI Discovery Behavior

The `enody` CLI discovers USB runtimes first unless `ENODY_DISABLE_USB` is set.
It then loads saved tokens and creates a `WifiEnvironment`, excluding host IDs
already present over USB. Most CLI control commands operate over the combined
runtime list, so the same command can work with products reached by USB or WiFi.
