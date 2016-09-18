// Copyright 2016 Google Inc. All Rights Reserved.
//
// Licensed under the MIT License, <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed except according to those terms.

use WireError;
use bincode::serde::DeserializeError;
use futures::{Async, BoxFuture, Future};
use futures::stream::Empty;
use std::fmt;
use std::io;
use tokio_proto::pipeline;
use tokio_service::Service;
use util::Never;

/// A client `Service` that writes and reads bytes.
///
/// Typically, this would be combined with a serialization pre-processing step
/// and a deserialization post-processing step.
pub struct Client<Req, Resp, E> {
    inner: pipeline::Client<Req,
                            Result<Result<Resp, WireError<E>>,
                                   DeserializeError>,
                            Empty<Never, io::Error>,
                            io::Error>,
}

impl<Req, Resp, E> Clone for Client<Req, Resp, E> {
    fn clone(&self) -> Self {
        Client { inner: self.inner.clone() }
    }
}

impl<Req, Resp, E> Service for Client<Req, Resp, E>
    where Req: Send + 'static,
          Resp: Send + 'static,
          E: Send + 'static,
{
    type Request = Req;
    type Response = Result<Resp, ::Error<E>>;
    type Error = io::Error;
    type Future = BoxFuture<Self::Response, Self::Error>;

    fn poll_ready(&self) -> Async<()> {
        Async::Ready(())
    }

    fn call(&self, request: Self::Request) -> Self::Future {
        self.inner.call(pipeline::Message::WithoutBody(request))
            .map(|r| r.map(|r| r.map_err(::Error::from))
                      .map_err(::Error::ClientDeserialize)
                      .and_then(|r| r))
            .boxed()
    }
}

impl<Req, Resp, E> fmt::Debug for Client<Req, Resp, E> {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "Client {{ .. }}")
    }
}

/// Exposes a trait for connecting asynchronously to servers.
pub mod future {
    use REMOTE;
    use futures::{self, Async, Future};
    use framed::Framed;
    use serde::{Deserialize, Serialize};
    use std::cell::RefCell;
    use std::io;
    use std::net::SocketAddr;
    use super::Client;
    use tokio_core::net::TcpStream;
    use tokio_proto::pipeline;


    /// Types that can connect to a server asynchronously.
    pub trait Connect: Sized {
        /// The type of the future returned when calling connect.
        type Fut: Future<Item = Self, Error = io::Error>;

        /// Connects to a server located at the given address.
        fn connect(addr: &SocketAddr) -> Self::Fut;
    }

    /// A future that resolves to a `Client` or an `io::Error`.
    pub struct ClientFuture<Req, Resp, E> {
        inner: futures::Oneshot<io::Result<Client<Req, Resp, E>>>,
    }

    impl<Req, Resp, E> Future for ClientFuture<Req, Resp, E> {
        type Item = Client<Req, Resp, E>;
        type Error = io::Error;

        fn poll(&mut self) -> futures::Poll<Self::Item, Self::Error> {
            match self.inner.poll().unwrap() {
                Async::Ready(Ok(client)) => Ok(Async::Ready(client)),
                Async::Ready(Err(err)) => Err(err),
                Async::NotReady => Ok(Async::NotReady),
            }
        }
    }

    impl<Req, Resp, E> Connect for Client<Req, Resp, E>
        where Req: Serialize + Send + 'static,
              Resp: Deserialize + Send + 'static,
              E: Deserialize + Send + 'static,
    {
        type Fut = ClientFuture<Req, Resp, E>;

        /// Starts an event loop on a thread and registers a new client
        /// connected to the given address.
        fn connect(addr: &SocketAddr) -> ClientFuture<Req, Resp, E> {
            let addr = *addr;
            let (tx, rx) = futures::oneshot();
            REMOTE.spawn(move |handle| {
                let handle2 = handle.clone();
                TcpStream::connect(&addr, handle)
                    .and_then(move |tcp| {
                        let tcp = RefCell::new(Some(tcp));
                        let c = try!(pipeline::connect(&handle2, move || {
                            Ok(Framed::new(tcp.borrow_mut().take().unwrap()))
                        }));
                        Ok(Client { inner: c })
                    })
                    .then(|client| Ok(tx.complete(client)))
            });
            ClientFuture { inner: rx }
        }
    }
}

/// Exposes a trait for connecting synchronously to servers.
pub mod sync {
    use futures::Future;
    use serde::{Deserialize, Serialize};
    use std::io;
    use std::net::ToSocketAddrs;
    use super::Client;

    /// Types that can connect to a server synchronously.
    pub trait Connect: Sized {
        /// Connects to a server located at the given address.
        fn connect<A>(addr: A) -> Result<Self, io::Error> where A: ToSocketAddrs;
    }

    impl<Req, Resp, E> Connect for Client<Req, Resp, E>
        where Req: Serialize + Send + 'static,
              Resp: Deserialize + Send + 'static,
              E: Deserialize + Send + 'static,
    {
        fn connect<A>(addr: A) -> Result<Self, io::Error>
            where A: ToSocketAddrs
        {
            let addr = if let Some(a) = try!(addr.to_socket_addrs()).next() {
                a
            } else {
                return Err(io::Error::new(io::ErrorKind::AddrNotAvailable,
                                          "`ToSocketAddrs::to_socket_addrs` returned an empty \
                                           iterator."));
            };
            <Self as super::future::Connect>::connect(&addr).wait()
        }
    }
}
