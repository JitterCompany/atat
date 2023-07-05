use super::AtatClient;
use crate::{
    helpers::LossyStr, response_channel::ResponseChannel, AtatCmd, Config, Error, Response,
};
// use embassy_time::{Duration};
use embedded_io::asynch::Write;

use fugit::ExtU64;
use fugit::{self, Duration};
use futures::future::{select, Either};
use futures::pin_mut;

pub trait AtTimerDriver {
    // type Instant;
    fn now(&self) -> fugit::Instant<u64, 1, 1000000>;
    async fn wait_instant(&self, instant: fugit::Instant<u64, 1, 1000000>);
}

// timer
pub struct Timer<D> {
    driver: D,
    delay: Option<fugit::Instant<u64, 1, 1000000>>,
}

impl<D: AtTimerDriver> Timer<D> {
    pub fn new(driver: D) -> Self {
        Self {
            driver,
            delay: None,
        }
    }

    fn start_delay(&mut self, delay_ms: u64) {
        let now = self.driver.now();
        let t_until = now
            .checked_add_duration(delay_ms.millis::<1, 1000>())
            .unwrap();
        // let delay = now.checked_add_duration(10u64.millis::<1,1000>()).unwrap();
        self.delay.replace(t_until);
        // Save instant at which time the delay may end.
    }

    async fn wait_delay(&mut self) {
        if let Some(delay) = self.delay {
            self.driver.wait_instant(delay).await
        }
        // Wait until saved instant has passed
    }
}

pub struct FWClient<'a, W: Write, const INGRESS_BUF_SIZE: usize, D> {
    writer: W,
    res_channel: &'a ResponseChannel<INGRESS_BUF_SIZE>,
    timer: Timer<D>,
    // config: Config,
    // cooldown_timer: Option<Timer>,
}

impl<'a, W: Write, const INGRESS_BUF_SIZE: usize, D: AtTimerDriver>
    FWClient<'a, W, INGRESS_BUF_SIZE, D>
{
    pub fn new(
        writer: W,
        res_channel: &'a ResponseChannel<INGRESS_BUF_SIZE>,
        timer: Timer<D>, // config: Config,
    ) -> Self {
        Self {
            writer,
            res_channel,
            timer, // config,
                   // cooldown_timer: None,
        }
    }

    async fn send_command(&mut self, cmd: &[u8]) -> Result<(), Error> {
        // wait until timeout

        self.wait_cooldown_timer().await;

        self.send_inner(cmd).await?;

        // save current time + delay
        self.start_cooldown_timer();
        Ok(())
    }

    async fn send_request(
        &mut self,
        cmd: &[u8],
        // timeout: Duration,
        timeout: u64,
    ) -> Result<Response<INGRESS_BUF_SIZE>, Error> {
        self.wait_cooldown_timer().await;

        let mut response_subscription = self.res_channel.subscriber().unwrap();
        self.send_inner(cmd).await?;

        let response = {
            let fut = response_subscription.next_message_pure();
            self.timer.start_delay(timeout);
            let timeout_fut = self.timer.wait_delay();
            pin_mut!(fut, timeout_fut);
            match select(fut, &mut timeout_fut).await {
                Either::Left((r, _right)) => Ok(r),
                Either::Right((_timeout, _left)) => Err(Error::Timeout),
            }
        };

        self.start_cooldown_timer();
        response
    }

    async fn send_inner(&mut self, cmd: &[u8]) -> Result<(), Error> {
        if cmd.len() < 50 {
            debug!("Sending command: {:?}", LossyStr(cmd));
        } else {
            debug!("Sending command with long payload ({} bytes)", cmd.len(),);
        }

        self.writer.write_all(cmd).await.map_err(|_| Error::Write)?;
        self.writer.flush().await.map_err(|_| Error::Write)?;
        Ok(())
    }

    fn start_cooldown_timer(&mut self) {
        // todo use config
        self.timer.start_delay(10);
        //     self.cooldown_timer = Some(Timer::after(self.config.cmd_cooldown));
    }

    async fn wait_cooldown_timer(&mut self) {
        self.timer.wait_delay().await;
    }
}

impl<W: Write, const INGRESS_BUF_SIZE: usize, D: AtTimerDriver> AtatClient
    for FWClient<'_, W, INGRESS_BUF_SIZE, D>
{
    async fn send<Cmd: AtatCmd<LEN>, const LEN: usize>(
        &mut self,
        cmd: &Cmd,
    ) -> Result<Cmd::Response, Error> {
        let cmd_vec = cmd.as_bytes();
        let cmd_slice = cmd.get_slice(&cmd_vec);
        if !Cmd::EXPECTS_RESPONSE_CODE {
            self.send_command(cmd_slice).await?;
            cmd.parse(Ok(&[]))
        } else {
            // let response = self
            //     .send_request(cmd_slice, Duration::from_millis(Cmd::MAX_TIMEOUT_MS.into()))
            //     .await?;

            let response = self
                .send_request(cmd_slice, Cmd::MAX_TIMEOUT_MS.into())
                .await?;
            cmd.parse((&response).into())
        }
    }
}
