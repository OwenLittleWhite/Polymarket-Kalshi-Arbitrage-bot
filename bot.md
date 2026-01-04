A few weeks ago, I decided to build my own @Polymarket bot. I spent several weeks developing a complete version of it.
I invested that time because there are many inefficiencies on Polymarket. Yes, it’s true that some bots already exploit these inefficiencies, but not enough. There are still far more opportunities than bots.
Bot Logic
The bot’s logic is based on a strategy I used to execute manually, which I then automated for efficiency. The bot operates on the “BTC 15-minute UP/DOWN” market.
The bot runs a live watcher that automatically switches to the current BTC 15-minute round, streams best bid/ask prices via WebSocket, displays a fixed terminal UI, and allows full control through text commands.
In manual mode, you can place trades directly.
buy up <usd> / buy down <usd> spends a dollar amount.
buyshares up <shares> / buyshares down <shares> buys an exact number of shares using a UI-friendly LIMIT + GTC order at the current best ask.
Auto mode runs a repeating two-leg cycle.
First, it watches price movements only during the first windowMin minutes of the round. If either side drops fast enough (a drop of at least movePct over roughly 3 seconds), it triggers Leg 1 and buys the side that dumped.
After Leg 1, the bot will never buy the same side again. It waits for Leg 2 (the hedge) and triggers it only when: leg1EntryPrice + oppositeAsk <= sumTarget.
When this condition is met, it buys the opposite side. After Leg 2 completes, the cycle is finished and the bot returns to watching for the next dump using the same parameters.
If the round changes during a cycle, the bot abandons the open cycle and restarts fresh on the next round with the same settings.
Auto mode parameters are set with: auto on <shares> [sum=0.95] [move=0.15] [windowMin=2]
shares: position size used for both legs
sum: threshold that allows the hedge
move (movePct): dump threshold (e.g. 0.15 = 15%)
windowMin: how long from the start of the round Leg 1 is allowed
Backtesting
The bot logic is simple: wait for a violent dump, buy the side that just dumped, then wait for stabilization and hedge by buying the opposite side while guaranteeing: priceUP + priceDOWN < 1
But this logic needed to be tested. Does it actually work over the long term?
More importantly, the bot has many parameters (shares, sum, movePct, windowMin, etc.). Which parameter sets are optimal and maximize profit?
My first idea was to run the bot live for one week and observe the results. The problem is that this takes too long and only allows testing one parameter set, whereas I needed to test many.
My second idea was to backtest using online historical data from Polymarket’s CLOB API. Unfortunately, for the BTC 15-minute UP/DOWN markets, the historical endpoints kept returning empty datasets. Without historical price ticks, the backtest can’t detect a “dump over ~3 seconds,” can’t trigger Leg 1, and produces 0 cycles and 0% ROI, regardless of the parameters.
After further investigation, I realized other users had the same issue retrieving historical data for certain markets. I tested other markets that did return historical data and concluded that, for this specific market, historical data is simply not retained.
Because of this limitation, the only reliable way to backtest this strategy was to create my own historical dataset by recording live best-ask prices while the bot was running.
The recorder writes snapshots to disk containing:
timestamp
round slug
seconds remaining
UP/DOWN token IDs
UP/DOWN best asks
The “recorded backtest” then replays these snapshots and applies the same auto logic deterministically. This guarantees access to the high-frequency data required to detect dumps and hedge conditions.
In total, I collected 6 GB of data over 4 days. I could have recorded more, but I felt this was sufficient to test different parameter sets.
I started to test this parameter set:
Initial balance: $1,000
20 shares per trade
sumTarget = 0.95
Dump threshold = 15%
windowMin = 2 minutes
I also applied a constant 0.5% fee rate and a 2% spread to stay in conservative scenarios.
The backtest shows an ROI of 86%, with $1,000 turning into $1,869 in just a few days.
I then tested riskier parameter set:
Initial balance: $1,000
20 shares per trade
sumTarget = 0.6
Dump threshold = 1%
windowMin = 15 minutes
Result: –50% ROI after 2 days.
This clearly shows that parameter selection is the most important factor. It can make you a lot of money, or cause significant losses.
Initial balance: $1,000
20 shares per trade
sumTarget = 0.6
Dump threshold = 1%
windowMin = 15 minutes
Result: –50% ROI after 2 days.
This clearly shows that parameter selection is the most important factor. It can make you a lot of money, or cause significant losses.
Backtest Limitations
Even with fees and spread included, the backtest has limitations.
First, it only uses a few days of data, which may not be enough for a full market view.
It relies on recorded best-ask snapshots; in reality, orders can be partially filled or filled at different prices. + Order book depth and available volume are not modeled.
Intra-second micro-movements are not captured (data is sampled once per second). Backtest has a 1s timestamp but a lot can happen between each second.
In the backtest, slippage is constant and does not simulate variable latency (e.g., 200–1500 ms) or network spikes.
Each leg is treated as an “instant” execution (no order queue, no resting orders).
Fees are applied uniformly, while in reality they may depend on:
> market/token
> maker vs taker
> fee tiers or conditions
To stay pessimistic, I applied a rule: if Leg 2 does not execute before market close, Leg 1 is treated as a total loss.
This is intentionally conservative but not always realistic:
sometimes Leg 1 can be closed early,
sometimes it finishes ITM and wins,
sometimes losses can be partial rather than total.
Losses may be overestimated, but this provides a useful “worst-case” scenario.
Most importantly, the backtest does not simulate your own market impact. In reality, your orders can:
move the order book,
attract or repel other traders,
cause nonlinear slippage.
The backtest assumes you are a pure price taker with no impact.
Finally, it does not simulate rate limits, API errors, order rejections, pauses, timeouts, reconnections, or moments when the bot is busy and misses signals.
Backtesting is extremely valuable for identifying good parameter ranges, but it is not a 100% guarantee, as some real-world effects cannot be modeled.
Infrastructure
I plan to run the bot on a Raspberry Pi to avoid consuming my main machine’s resources and to keep it running 24/7. 
But this can be significantly improved.
Using Rust instead of JavaScript would provide far better performance and processing times.
Running a dedicated Polygon RPC node would reduce latency further.
Deploying on a VPS close to Polymarket’s servers would also significantly reduce latency.
There are certainly other optimizations I haven’t discovered yet.
For now, I’m learning Rust, because it’s becoming an essential language for Web3 development.
If you’re interested in my bot, which also includes the backtesting framework, DM me.