use codec::{Decode, Encode};

use futures::StreamExt;
use libp2p::{
    development_transport,
    identify::{Identify, IdentifyEvent, IdentifyInfo},
    identity,
    identity::{ed25519::Keypair, PublicKey},
    kad::record::store::MemoryStore,
    kad::{GetClosestPeersError, Kademlia, KademliaConfig, KademliaEvent, QueryResult},
    swarm::{NetworkBehaviourEventProcess, Swarm},
    Multiaddr, NetworkBehaviour, PeerId,
};
use std::{collections::HashSet, time::Duration};
use std::{error::Error, str::FromStr};

const BOOTNODE_ID: usize = 100;
const USAGE_MSG: &str = "Missing arg. Usage
    cargo run --example honest my_id n_members n_finalized

    my_id -- our index
    n_members -- size of the committee
    n_finalized -- number of data to be finalized";

fn parse_arg(n: usize) -> usize {
    if let Some(int) = std::env::args().nth(n) {
        match int.parse::<usize>() {
            Ok(int) => int,
            Err(err) => {
                panic!("Failed to parse arg {:?}", err);
            }
        }
    } else {
        panic!("{}", USAGE_MSG);
    }
}

fn generate_keys() -> Vec<Keypair> {
    let keypairs = (0..101).map(|_| Keypair::generate()).collect::<Vec<_>>();
    println!(
        "bootnode: {:?}",
        PeerId::from_public_key(PublicKey::Ed25519(keypairs[100].public()))
    );

    let data = keypairs
        .iter()
        .map(|kp| kp.encode())
        .collect::<Vec<_>>()
        .encode();
    std::fs::write("examples/kad/keypairs", data).expect("Unable to write file");

    keypairs
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init();

    let keypairs = match std::fs::read("examples/kad/keypairs") {
        Ok(data) => Vec::<[u8; 64]>::decode(&mut &data[..])
            .expect("Kepairs are properly encoded")
            .iter_mut()
            .map(|kp| Keypair::decode(&mut kp[..]).expect("keypair is properly encoded"))
            .collect(),
        Err(_) => generate_keys(),
    };

    let my_id = parse_arg(1);
    assert!(my_id <= 100);

    let local_key = identity::Keypair::Ed25519(keypairs[my_id].clone());
    let local_peer_id = PeerId::from_public_key(local_key.public());

    println!("My peer id: {:?}", local_peer_id);

    // Set up a an encrypted DNS-enabled TCP Transport over the Mplex protocol
    let transport = development_transport(local_key).await?;

    // Create a swarm to manage peers and events.
    let mut swarm = {
        // Create a Kademlia behaviour.
        let cfg = KademliaConfig::default();
        // cfg.set_query_timeout(Duration::from_secs(5 * 60));
        let store = MemoryStore::new(local_peer_id);
        let behaviour = Kademlia::with_config(local_peer_id, store, cfg);

        Swarm::new(transport, behaviour, local_peer_id)
    };

    let addr = format!("/ip4/127.0.0.1/tcp/{}", 10000 + my_id);
    swarm.listen_on(addr.parse()?)?;

    if my_id == BOOTNODE_ID {
        loop {
            println!("loop {:?}", swarm.next().await);
        }
    } else {
        // This probably works only on localhost?
        let bootnode = PeerId::from_public_key(PublicKey::Ed25519(keypairs[BOOTNODE_ID].public()));
        println!("bootnode {:?}", bootnode);

        let bootaddr = Multiaddr::from_str("/ip4/127.0.0.1/tcp/10100")?;
        swarm
            .behaviour_mut()
            .add_address(&bootnode, bootaddr.clone());

        println!("Bootstrap {:?}", swarm.behaviour_mut().bootstrap());

        // Kick it off!
        tokio::spawn(async move {
            // println!(
            //     "start providing {:?}",
            //     swarm.behaviour_mut().start_providing(Key::new(&[0]))
            // );
            loop {
                // Order Kademlia to search for a peer.
                let to_search: PeerId = identity::Keypair::generate_ed25519().public().into();
                println!("Searching for the closest peers to {:?}", to_search);
                swarm.behaviour_mut().get_closest_peers(to_search);

                let event = swarm.select_next_some().await;
                println!("{:?}", event);

                futures_timer::Delay::new(Duration::from_secs(1)).await;

                break;
            }

            ()
        })
        .await?;
    };

    Ok(())
}

#[derive(NetworkBehaviour)]
struct Behaviour {
    kad: Kademlia<MemoryStore>,
    identify: Identify,
    #[behaviour(ignore)]
    unroutable_peers: HashSet<PeerId>,
    #[behaviour(ignore)]
    discovered_peers: HashSet<PeerId>,
}

impl NetworkBehaviourEventProcess<KademliaEvent> for Behaviour {
    fn inject_event(&mut self, event: KademliaEvent) {
        if let KademliaEvent::QueryResult {
            result: QueryResult::GetClosestPeers(result),
            ..
        } = event
        {
            if let Ok(ok) = result {
                println!("Query finished with closest peers: {:#?}", ok.peers);
                let unroutable_peers = self.add_unroutable_peers(&ok.peers);
                self.identify.push(unroutable_peers);
            };
        }
    }
}

impl NetworkBehaviourEventProcess<IdentifyEvent> for Behaviour {
    fn inject_event(&mut self, event: IdentifyEvent) {
        if let IdentifyEvent::Received { peer_id, info } = event {
            self.unroutable_peers.remove(&peer_id);
            if self.discovered_peers.insert(peer_id) {
                println!("Discovered peer: {:?}", peer_id);
                self.kad.add_address(&peer_id, info.observed_addr);
            }
        }
    }
}

impl Behaviour {
    fn add_unroutable_peers(&mut self, peers: &Vec<PeerId>) -> HashSet<PeerId> {
        let mut unroutable_peers = HashSet::new();
        for peer in peers.iter() {
            if self.discovered_peers.contains(peer) {
                continue;
            }
            if !self.unroutable_peers.contains(peer) {
                unroutable_peers.insert(*peer);
                self.unroutable_peers.insert(*peer);
            }
        }

        unroutable_peers
    }
}
