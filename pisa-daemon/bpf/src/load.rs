// Copyright 2022 SphereEx Authors
// 
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
// 
//     http://www.apache.org/licenses/LICENSE-2.0
// 
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::convert::{TryFrom, TryInto};
use std::io::{ErrorKind, Error};
use std::os::unix::io::AsRawFd;
use std::path::Path;
use aya::programs::{SocketFilter, tc, SchedClassifier, TcAttachType};
use aya::maps::{ProgramArray, HashMap, MapRefMut};
use aya::{Bpf, Pod, BpfLoader};

use socket2::{Socket, Domain, Type, Protocol};

pub enum LoadSockFilter {
    App,
    SQL,
}

impl LoadSockFilter {
    fn open_device(device: String) -> Result<Socket, Box<dyn std::error::Error>> {
        let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
        socket.bind_device(Some(device.as_bytes()))?;
        Ok(socket)
    }

    fn load_app_filter<P: AsRef<Path>>(&self, device: String, path: P) -> Result<(), Box<dyn std::error::Error>> {
        let mut bpf = Bpf::load_file(path)?;
        let prog: &mut SocketFilter = bpf.program_mut("app").unwrap().try_into()?;
        prog.load()?;

        let socket = Self::open_device(device)?;
        prog.attach(socket.as_raw_fd())?;

        Ok(())
    }

}


#[derive(Debug, Clone, Copy)]
struct Endpoint {
	ip: u32,
    port: u16,
}

// Implements `Pod` to `Endpoint` as Map Key.
unsafe impl Pod for Endpoint { }

pub enum TrafficTyp {
    App,
    SQL,
}

pub const APP_ENDPOINTS_CLASSID_PIN_PATH: &str = "/sys/fs/bpf/pisa-daemon/app-endpoints-classid";
pub const APP_ENDPOINTS_CLASSID_MAP_NAME: &str = "app_endpoints_classid";

impl TrafficTyp {
    fn add_clsact(&self, device: &str) -> Result<(), Box<dyn std::error::Error>> {
        if let Err(e) = tc::qdisc_add_clsact(&device) {
            if e.kind() != ErrorKind::AlreadyExists {
                return Err(Box::new(e))
            }
        }
        Ok(())
    }

    pub fn load<P: AsRef<Path>>(&self, path: P, device: &str) -> Result<Bpf, Box<dyn std::error::Error>> {
        self.add_clsact(device)?;
        match self {
            Self::App => {
                self.app(path, device)
            },

            Self::SQL => todo!()
        }
    }

    fn app<P: AsRef<Path>>(&self, path: P, device: &str) ->  Result<Bpf, Box<dyn std::error::Error>> {
        if !Path::new(APP_ENDPOINTS_CLASSID_PIN_PATH).exists() {
            let _ = std::fs::create_dir_all(APP_ENDPOINTS_CLASSID_PIN_PATH);
        }

        let mut bpf = BpfLoader::new().map_pin_path(APP_ENDPOINTS_CLASSID_PIN_PATH).load_file(path)?;
        let prog: &mut SchedClassifier = bpf.program_mut("classifier").unwrap().try_into()?;
        
        prog.load()?;
        prog.attach(&device, TcAttachType::Egress)?;
        Ok(bpf)
    }

    // Todo, need to add config parameter
    pub fn load_app_config(&self, bpf: &mut Bpf) -> Result<MapRefMut, Box<(dyn std::error::Error + 'static)>> {
        bpf.map_mut(APP_ENDPOINTS_CLASSID_MAP_NAME).map_err(|e| e.into())
    }
}

#[cfg(test)]
mod test {
    use std::process::Command;

    use aya::maps::HashMap;

    use crate::load::{Endpoint, TrafficTyp};

    #[test]
    fn test_load_app_config() {
        let _ = Command::new("clang").args("-O2 -target bpf -g -c tc/app.c -o tc/app.o -I ./".split(" ")).spawn();
        let load = TrafficTyp::App;
        load.add_clsact("lo");
        let try_bpf = load.app("tc/app.o", "lo");
        assert_eq!(try_bpf.is_err(), false);
        let bpf = try_bpf.unwrap();
        let mut map = HashMap::<_, Endpoint, u32>::try_from(bpf.map_mut("app_endpoints_classid").unwrap()).unwrap();
        
        let ep = Endpoint { ip: 11111, port: 8000 };
        map.insert(ep, 1, 0).unwrap();

        let v = map.get(&ep, 0).unwrap();
        assert_eq!(v, 1);
    }
}
