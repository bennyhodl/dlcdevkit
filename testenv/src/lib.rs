//! Managed regtest backends for the DDK test suites.
//!
//! Every integration test used to point at a docker-compose stack over fixed
//! ports (`18443` for bitcoind, `30000` for electrs) and read its connection
//! details out of the environment. This crate replaces that: each test binary
//! launches its own bitcoind and electrs on ephemeral ports, so `cargo test`
//! needs nothing running beforehand.
//!
//! There are two ways to get backends, and the choice matters:
//!
//! [`env`] returns one bitcoind/electrs pair per test binary, shared by every
//! test in it. Cheap, and fine for tests that only need somewhere to put coins.
//!
//! ```no_run
//! let env = ddk_testenv::env();
//! env.generate_blocks(5);
//! let esplora = env.esplora_host();
//! ```
//!
//! [`TestEnv::new`] returns a private pair, torn down when dropped. Tests that
//! assert on contract state as blocks advance need this: libtest runs tests
//! concurrently, and on a shared chain the blocks a sibling test mines can push
//! a contract past a locktime before the assertion runs.
//!
//! ```no_run
//! let env = ddk_testenv::TestEnv::new();
//! let esplora = env.esplora_host();
//! ```

use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use bitcoin::{Address, Amount, Txid};
use bitcoincore_rpc::{jsonrpc, Client, RpcApi};

pub use bitcoincore_rpc;
pub use bitcoind;
pub use electrsd;

#[cfg(feature = "nostr")]
pub mod nostr;
#[cfg(feature = "postgres")]
pub mod postgres;

/// Blocks mined during boot, so coinbase outputs are spendable immediately.
const MATURITY_BLOCKS: u64 = 101;

/// Read timeout for the test RPC clients.
///
/// Well above jsonrpc's 15s default. Several tests mine a hundred blocks in a
/// single `generatetoaddress` call, and a dozen of them run at once, so a
/// request can sit behind a lot of work before the node answers it.
const RPC_TIMEOUT: Duration = Duration::from_secs(300);

/// A regtest bitcoind paired with an electrs serving the esplora HTTP API.
///
/// The two child processes are owned by [`Mutex`]es purely so [`shutdown`] can
/// drop them from the process-exit hook; nothing else needs the interior
/// mutability.
///
/// [`shutdown`]: TestEnv::shutdown
pub struct TestEnv {
    bitcoind: Mutex<Option<bitcoind::BitcoinD>>,
    electrsd: Mutex<Option<electrsd::ElectrsD>>,
    esplora_host: String,
    rpc_url: String,
    rpc_user: String,
    rpc_password: String,
    mine_address: Address,
}

static ENV: OnceLock<TestEnv> = OnceLock::new();

/// Returns backends shared by every test in this binary, booting them on first
/// call.
///
/// Suitable for test binaries holding a handful of tests that do not care what
/// else is happening on the chain. Tests that assert on contract state as
/// blocks advance need [`TestEnv::new`] instead: blocks another test mines are
/// visible to all of them, which can drive a contract past a locktime early.
pub fn env() -> &'static TestEnv {
    ENV.get_or_init(|| {
        // libtest exits the process via `std::process::exit`, which never runs
        // destructors for statics. Without this hook the bitcoind and electrs
        // children outlive the test binary and are left behind as orphans.
        unsafe { libc::atexit(shutdown_at_exit) };
        TestEnv::boot()
    })
}

extern "C" fn shutdown_at_exit() {
    if let Some(env) = ENV.get() {
        env.shutdown();
    }
}

/// Locks through poisoning: a panicking test must not prevent process cleanup.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Builds an RPC client by hand rather than through [`Client::new`], so the
/// transport gets [`RPC_TIMEOUT`] instead of jsonrpc's default.
fn rpc_client(url: &str, user: &str, password: &str) -> Client {
    let transport = jsonrpc::simple_http::Builder::new()
        .url(url)
        .expect("bitcoind RPC url is malformed")
        .auth(user.to_string(), Some(password.to_string()))
        .timeout(RPC_TIMEOUT)
        .build();
    Client::from_jsonrpc(jsonrpc::Client::with_transport(transport))
}

impl TestEnv {
    /// Starts backends used by nothing else, torn down when the value is
    /// dropped.
    ///
    /// Costs a few seconds, which buys a private chain: a test holding its own
    /// environment cannot be perturbed by blocks other tests mine.
    pub fn new() -> TestEnv {
        TestEnv::boot()
    }

    fn boot() -> TestEnv {
        let mut conf = bitcoind::Conf::default();
        conf.args.push("-txindex=1");
        conf.args.push("-addresstype=bech32");
        conf.args.push("-fallbackfee=0.0002");
        // libtest runs every test in a binary concurrently and they all share
        // this node. The defaults (4 threads, a 16-deep queue) drop requests
        // under that load, surfacing as `WouldBlock` transport errors.
        conf.args.push("-rpcthreads=32");
        conf.args.push("-rpcworkqueue=1024");
        conf.wallet = Some("ddk".to_string());
        let bitcoind = bitcoind::BitcoinD::with_conf(
            bitcoind::exe_path().expect("no bitcoind executable available"),
            &conf,
        )
        .expect("failed to start bitcoind");

        let cookie = bitcoind
            .params
            .get_cookie_values()
            .expect("failed to read the bitcoind cookie file")
            .expect("bitcoind cookie file was empty");
        let rpc_url = format!("http://{}/wallet/ddk", bitcoind.params.rpc_socket);
        let rpc = rpc_client(&rpc_url, &cookie.user, &cookie.password);

        let mine_address = rpc
            .get_new_address(None, None)
            .expect("RPC error")
            .assume_checked();
        rpc.generate_to_address(MATURITY_BLOCKS, &mine_address)
            .expect("RPC error");

        // electrs is started after the chain exists so its initial sync covers
        // the mature coinbases in one pass.
        let mut electrs_conf = electrsd::Conf::default();
        electrs_conf.http_enabled = true;
        electrs_conf.network = "regtest";
        let electrsd = electrsd::ElectrsD::with_conf(
            electrsd::exe_path().expect("no electrs executable available"),
            &bitcoind,
            &electrs_conf,
        )
        .expect("failed to start electrs");

        // `esplora_url` is the bind address electrs was given (`0.0.0.0:port`),
        // which is not a valid address to connect to.
        let esplora_host = format!(
            "http://{}",
            electrsd
                .esplora_url
                .as_ref()
                .expect("electrs was started with http_enabled")
                .replace("0.0.0.0", "127.0.0.1")
        );

        TestEnv {
            bitcoind: Mutex::new(Some(bitcoind)),
            electrsd: Mutex::new(Some(electrsd)),
            esplora_host,
            rpc_url,
            rpc_user: cookie.user,
            rpc_password: cookie.password,
            mine_address,
        }
    }

    /// Base URL of the esplora HTTP API backed by this environment's chain.
    pub fn esplora_host(&self) -> &str {
        &self.esplora_host
    }

    /// A fresh RPC client bound to bitcoind's `ddk` wallet.
    pub fn rpc(&self) -> Client {
        rpc_client(&self.rpc_url, &self.rpc_user, &self.rpc_password)
    }

    /// Mines `count` blocks and returns once electrs has indexed the new tip.
    ///
    /// Waiting on the indexer is what makes the fixed sleeps the old suite used
    /// unnecessary: when this returns, a wallet syncing against esplora will
    /// see the blocks.
    pub fn generate_blocks(&self, count: u64) {
        let rpc = self.rpc();
        let target = rpc.get_block_count().expect("RPC error") + count;
        rpc.generate_to_address(count, &self.mine_address)
            .expect("RPC error");
        self.wait_for_height(target);
    }

    /// Blocks until electrs has indexed up to `height`.
    pub fn wait_for_height(&self, height: u64) {
        let guard = lock(&self.electrsd);
        let electrsd = guard.as_ref().expect("electrs was shut down");
        let _ = electrsd.trigger();
        electrsd.wait_height(height as usize);
    }

    /// Blocks until electrs has indexed `txid` and its output scripts.
    pub fn wait_for_tx(&self, txid: &Txid) {
        let guard = lock(&self.electrsd);
        let electrsd = guard.as_ref().expect("electrs was shut down");
        let _ = electrsd.trigger();
        electrsd.wait_tx(txid);
    }

    /// Sends `amount` to `address` from bitcoind's wallet, without confirming it.
    pub fn send_to_address(&self, address: &Address, amount: Amount) -> Txid {
        self.rpc()
            .send_to_address(address, amount, None, None, None, None, None, None)
            .expect("RPC error")
    }

    /// Sends `amount` to `address` and confirms it, leaving the funds spendable.
    pub fn fund_address(&self, address: &Address, amount: Amount) -> Txid {
        let txid = self.send_to_address(address, amount);
        self.generate_blocks(1);
        txid
    }

    /// Terminates both child processes. Idempotent.
    fn shutdown(&self) {
        // electrs first: it polls bitcoind and logs noisily if the daemon
        // disappears from under it.
        lock(&self.electrsd).take();
        lock(&self.bitcoind).take();
    }
}

impl Default for TestEnv {
    fn default() -> Self {
        TestEnv::new()
    }
}

/// Cleans up environments created with [`TestEnv::new`]. The shared one from
/// [`env`] is a static and never dropped; the process-exit hook covers it.
impl Drop for TestEnv {
    fn drop(&mut self) {
        self.shutdown();
    }
}
