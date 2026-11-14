//! Two nodes joined by an in-memory "cable": QUIC runs over the link, and a
//! live connection picks the link up and drops it again.

#![cfg(feature = "iroh")]

use std::time::Duration;

use aurora_transport::{Connection, LinkKind, Node, Via, via};
use iroh::SecretKey;
use tokio::{task::JoinHandle, time::sleep};

async fn node() -> Node {
  Node::bind(SecretKey::generate(), None, false)
    .await
    .unwrap()
}

/// Plugs a cable between `a` and `b`; abort the handle to unplug it
fn cable(a: &Node, b: &Node) -> JoinHandle<()> {
  let (x, y) = tokio::io::duplex(1 << 16);
  let (xr, xw) = tokio::io::split(x);
  let (yr, yw) = tokio::io::split(y);
  let (la, lb) = (a.links().clone(), b.links().clone());
  tokio::spawn(async move {
    let _ = tokio::join!(
      la.attach(LinkKind::Usb, xr, xw),
      lb.attach(LinkKind::Usb, yr, yw)
    );
  })
}

/// Echoes one stream per connection
fn serve(node: &Node) {
  let node = node.clone();
  tokio::spawn(async move {
    while let Some(Ok(conn)) = node.accept().await {
      tokio::spawn(async move {
        while let Ok((mut send, mut recv)) = conn.accept_bi().await {
          let data = recv.read_to_end(1 << 16).await.unwrap();
          send.write_all(&data).await.unwrap();
          send.finish().unwrap();
        }
      });
    }
  });
}

async fn echo(conn: &Connection, msg: &[u8]) {
  let (mut send, mut recv) = conn.open_bi().await.unwrap();
  send.write_all(msg).await.unwrap();
  send.finish().unwrap();
  assert_eq!(recv.read_to_end(1 << 16).await.unwrap(), msg);
}

async fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
  for _ in 0..100 {
    if f() {
      return;
    }
    sleep(Duration::from_millis(100)).await;
  }
  panic!("timed out waiting for {what}");
}

#[tokio::test(flavor = "multi_thread")]
async fn dials_over_the_cable_alone() {
  let (a, b) = (node().await, node().await);
  serve(&b);
  let _cable = cable(&a, &b);
  wait_for("link up", || a.links().is_linked(&b.id())).await;
  // no addresses: only the link lookup knows where `b` is
  let conn = a.connect_link(b.id()).await.unwrap();
  assert_eq!(via(&conn), Via::Link(LinkKind::Usb));
  echo(&conn, b"over usb").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn network_and_cable_connections_live_side_by_side() {
  let (a, b) = (node().await, node().await);
  serve(&b);
  let ip = a.connect(b.id(), &b.hints()).await.unwrap();
  echo(&ip, b"over ip").await;

  let cable = cable(&a, &b);
  wait_for("link up", || a.links().is_linked(&b.id())).await;
  let usb = a.dial(b.id(), &b.hints()).await.unwrap();
  assert_eq!(via(&usb), Via::Link(LinkKind::Usb));
  echo(&usb, b"over usb").await;
  echo(&ip, b"still over ip").await;

  cable.abort();
  tokio::time::timeout(Duration::from_secs(5), a.links().gone(&b.id()))
    .await
    .expect("link reported gone");
  usb.close(0u32.into(), b"unplugged");
  echo(&ip, b"ip unaffected").await;
  assert!(matches!(
    via(&a.dial(b.id(), &b.hints()).await.unwrap()),
    Via::Direct(_)
  ));
}

#[tokio::test(flavor = "multi_thread")]
async fn connect_link_needs_a_link() {
  let (a, b) = (node().await, node().await);
  assert!(a.connect_link(b.id()).await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn link_events_reach_seen() {
  let (a, b) = (node().await, node().await);
  let mut seen = a.seen();
  let cable = cable(&a, &b);
  match seen.recv().await.unwrap() {
    aurora_transport::Seen::Found { id, via, .. } => {
      assert_eq!(id, b.id());
      assert_eq!(via, Via::Link(LinkKind::Usb));
    }
    other => panic!("{other:?}"),
  }
  cable.abort();
  assert_eq!(
    seen.recv().await.unwrap(),
    aurora_transport::Seen::Gone(b.id())
  );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn accessory_fd_carries_a_link() {
  use std::os::{fd::OwnedFd, unix::net::UnixStream};

  let (a, b) = (node().await, node().await);
  serve(&b);
  let (phone, host) = UnixStream::pair().unwrap();
  let phone_links = a.links().clone();
  tokio::spawn(async move {
    let _ = aurora_transport::usb::attach_fd(&phone_links, OwnedFd::from(phone)).await;
  });
  host.set_nonblocking(true).unwrap();
  let host = tokio::net::UnixStream::from_std(host).unwrap();
  let (hr, hw) = host.into_split();
  let host_links = b.links().clone();
  let cable = tokio::spawn(async move {
    let _ = host_links.attach(LinkKind::Usb, hr, hw).await;
  });
  wait_for("link up", || a.links().is_linked(&b.id())).await;
  let conn = a.connect_link(b.id()).await.unwrap();
  echo(&conn, &[5u8; 40_000]).await;

  cable.abort();
  tokio::time::timeout(Duration::from_secs(5), a.links().gone(&b.id()))
    .await
    .expect("phone side sees the cable go");
}

#[tokio::test(flavor = "multi_thread")]
async fn waiting_end_skips_stale_bytes_and_answers_a_restarted_host() {
  use aurora_transport::link::frame;
  use tokio::io::AsyncWriteExt;

  let phone = node().await;
  let (x, y) = tokio::io::duplex(1 << 16);
  let (xr, xw) = tokio::io::split(x);
  let links = phone.links().clone();
  tokio::spawn(async move {
    let _ = links.attach_waiting(LinkKind::Usb, xr, xw).await;
  });
  let (hr, mut hw) = tokio::io::split(y);
  let mut hr = frame::FrameReader::new(hr);
  let host: aurora_proto::Id = [7; 32];
  // longer than the plain handshake timeout: the phone keeps waiting
  sleep(Duration::from_secs(6)).await;
  hw.write_all(b"\x13\x37 tail of an old session AU\xff\x00")
    .await
    .unwrap();
  frame::write_hello(&mut hw, &host, true).await.unwrap();
  // the phone's own hello, then its answer to ours
  assert_eq!(next_hello(&mut hr).await, (phone.id(), true));
  assert_eq!(next_hello(&mut hr).await, (phone.id(), false));
  wait_for("link up", || phone.links().is_linked(&host)).await;

  // the host restarts its end of the cable without unplugging
  frame::write_hello(&mut hw, &host, true).await.unwrap();
  assert_eq!(next_hello(&mut hr).await, (phone.id(), false));
  assert!(phone.links().is_linked(&host));
}

async fn next_hello<R: tokio::io::AsyncRead + Unpin>(
  r: &mut aurora_transport::link::frame::FrameReader<R>,
) -> (aurora_proto::Id, bool) {
  use aurora_transport::link::frame::Frame;
  loop {
    if let Some(Frame::Hello { id, reply }) = r.next().await.unwrap() {
      return (id, reply);
    }
  }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn accessory_follows_a_new_host_on_the_same_cable() {
  use std::os::{fd::OwnedFd, unix::net::UnixStream};

  use aurora_transport::link::frame;

  let phone = node().await;
  let (phone_end, host_end) = UnixStream::pair().unwrap();
  let links = phone.links().clone();
  let done = tokio::spawn(async move {
    aurora_transport::usb::attach_fd(&links, OwnedFd::from(phone_end)).await
  });
  host_end.set_nonblocking(true).unwrap();
  let (hr, mut hw) = tokio::net::UnixStream::from_std(host_end)
    .unwrap()
    .into_split();
  let mut hr = frame::FrameReader::new(hr);

  let (old, new): (aurora_proto::Id, aurora_proto::Id) = ([1; 32], [2; 32]);
  frame::write_hello(&mut hw, &old, true).await.unwrap();
  wait_for("old host", || phone.links().is_linked(&old)).await;

  // a different daemon takes over the cable; it repeats its hello until
  // answered, like the real host does
  let answered = async {
    loop {
      frame::write_hello(&mut hw, &new, true).await.unwrap();
      let got = tokio::time::timeout(Duration::from_millis(500), next_hello(&mut hr)).await;
      if let Ok((id, false)) = got {
        assert_eq!(id, phone.id());
        return;
      }
    }
  };
  tokio::time::timeout(Duration::from_secs(10), answered)
    .await
    .expect("phone answers the new host");
  wait_for("new host", || phone.links().is_linked(&new)).await;
  assert!(!phone.links().is_linked(&old));

  // unplugged: the fd threads end and attach_fd returns
  drop((hr, hw));
  tokio::time::timeout(Duration::from_secs(5), done)
    .await
    .expect("attach_fd ends with the cable")
    .unwrap()
    .unwrap();
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn dropping_the_accessory_link_frees_a_blocked_fd() {
  use std::{
    io::Read,
    os::{fd::OwnedFd, unix::net::UnixStream},
  };

  let phone = node().await;
  // the host never sends: the phone's read thread blocks, like acc_read
  // after the cable is pulled on some kernels
  let (phone_end, mut host_end) = UnixStream::pair().unwrap();
  let links = phone.links().clone();
  let task = tokio::spawn(async move {
    aurora_transport::usb::attach_fd(&links, OwnedFd::from(phone_end)).await
  });
  sleep(Duration::from_millis(300)).await;
  task.abort();

  // both of the phone's copies of the fd close: the host reads end of file
  host_end
    .set_read_timeout(Some(Duration::from_secs(5)))
    .unwrap();
  let mut buf = Vec::new();
  let res = tokio::task::spawn_blocking(move || host_end.read_to_end(&mut buf).map(|_| buf))
    .await
    .unwrap();
  let hello = res.expect("fd closed, not timed out");
  assert!(hello.starts_with(b"AU"), "only the phone's hello was sent");
}
