"""Inspired by https://github.com/python-telegram-bot/python-telegram-bot/blob/master/examples/timerbot.py"""

import re
from datetime import datetime
import requests
from pathlib import Path
import bs4
from loguru import logger
from omegaconf import OmegaConf
from telegram import Update
from telegram.ext import (Application, ApplicationBuilder, MessageHandler,
                          CommandHandler, ContextTypes, filters)


TG_BOT_TOKEN = ""  # ACT: set with the bot token obtained from @BotFather

TG_POLL_INTERVAL = 10  # in seconds, how freq-ly to poll user input from the bot
DB_SAVE_INTERVAL = 3600  # in seconds, how freq-ly to save the db
QUERY_EXEC_INTERVAL = 3600  # in seconds, how freq-ly to exec the queries
JOB_GRACE_TIME = 300  # in seconds
PATH_DB = Path("./db.yaml")


async def cb_start(update: Update, context: ContextTypes.DEFAULT_TYPE) -> None:
    user_id = update.effective_user.id
    chat_id = update.effective_message.chat_id
    if user_id not in context.bot_data["db"]:
        context.bot_data["db"][user_id] = {"chat_id": chat_id,
                                           "qs": [],
                                           "dtimes_prev_req": []}
        await context.bot.send_message(
            chat_id,
            f"\U0001F64B Welcome to the service! Your user ID is {user_id}",
            parse_mode="Markdown")
    else:
        await context.bot.send_message(
            chat_id,
            f"\U0001F64B You are already registered under ID {user_id}",
            parse_mode="Markdown")


async def cb_query_add(update: Update, context: ContextTypes.DEFAULT_TYPE) -> None:
    user_id = update.effective_user.id
    chat_id = update.effective_chat.id
    if len(context.args) == 0:
        await context.bot.send_message(
            chat_id,
            "\U00002757 Cannot create an empty query",
            parse_mode="Markdown")
    else:
        q = "+".join(context.args)
        if q not in context.bot_data["db"][user_id]["qs"]:
            context.bot_data["db"][user_id]["qs"].append(q)
            context.bot_data["db"][user_id]["dtimes_prev_req"].append("never")
            await schedule_query(job_queue=context.job_queue, user_id=user_id,
                                 chat_id=chat_id, query=q)
            await context.bot.send_message(
                chat_id,
                f"\U00002705 Query added: *{q}*",
                parse_mode="Markdown")
        else:
            await context.bot.send_message(
                chat_id,
                f"\U00002757 Query already exists: *{q}*",
                parse_mode="Markdown")


async def cb_query_remove(update: Update, context: ContextTypes.DEFAULT_TYPE) -> None:
    user_id = update.effective_user.id
    chat_id = update.effective_chat.id
    if len(context.args) == 0:
        await context.bot.send_message(
            chat_id,
            "\U00002757 Specify query index",
            parse_mode="Markdown")
    else:
        try:
            i = int(context.args[0])
        except ValueError as e:
            await context.bot.send_message(
                chat_id,
                f"\U00002757 Incorrect index: {context.args[0]}",
                parse_mode="Markdown")
            return
        qs = context.bot_data["db"][user_id]["qs"]
        if i in range(len(qs)):
            q = qs[i]
            context.bot_data["db"][user_id]["qs"].pop(i)
            context.bot_data["db"][user_id]["dtimes_prev_req"].pop(i)
            await cancel_query(job_queue=context.job_queue, user_id=user_id,
                               chat_id=chat_id, query=q)
            await context.bot.send_message(
                chat_id,
                f"\U0000274E Query removed: {i} (*{q}*)",
                parse_mode="Markdown")
        else:
            await context.bot.send_message(
                chat_id,
                f"\U00002757 Incorrect index: {context.args[0]}",
                parse_mode="Markdown")


async def cb_query_list(update: Update, context: ContextTypes.DEFAULT_TYPE) -> None:
    user_id = update.effective_user.id
    chat_id = update.effective_chat.id
    qs = context.bot_data["db"][user_id]["qs"]
    ts = context.bot_data["db"][user_id]["dtimes_prev_req"]
    if len(qs):
        msg = "\U0001F4CB Existing queries:"
        for i, (q, t) in enumerate(zip(qs, ts)):
            msg += (f"\n[{i}]: *{q}* (upd: {t})")
    else:
        msg = "\U0001F4CB No queries found"
    await context.bot.send_message(chat_id, msg, parse_mode="Markdown")


async def cb_query_clear(update: Update, context: ContextTypes.DEFAULT_TYPE) -> None:
    user_id = update.effective_user.id
    chat_id = update.effective_chat.id
    qs = context.bot_data["db"][user_id]["qs"]
    for q in qs:
        await cancel_query(job_queue=context.job_queue, user_id=user_id,
                           chat_id=chat_id, query=q)
    context.bot_data["db"][user_id]["qs"].clear()
    context.bot_data["db"][user_id]["ts_upd"].clear()
    await context.bot.send_message(chat_id, "\U0001F6BD All queries cleared",
                                   parse_mode="Markdown")


async def cb_stop(update: Update, context: ContextTypes.DEFAULT_TYPE) -> None:
    user_id = update.effective_user.id
    chat_id = update.effective_message.chat_id
    if user_id in context.bot_data["db"]:
        context.bot_data["db"][user_id]["qs"].clear()
        context.bot_data["db"][user_id]["dtimes_prev_req"].clear()
        del context.bot_data["db"][user_id]
        await context.bot.send_message(
            chat_id,
            "\U0001F64B You have been unregistered",
            parse_mode="Markdown")
    else:
        await context.bot.send_message(
            chat_id,
            f"\U0001F64B No user found with ID {user_id}",
            parse_mode="Markdown")


async def cb_msg(update: Update, context: ContextTypes.DEFAULT_TYPE) -> None:
    chat_id = update.effective_chat.id
    await context.bot.send_message(chat_id, "\U00002755 See the commands menu",
                                   parse_mode="Markdown")


async def job_save_db(context: ContextTypes.DEFAULT_TYPE) -> None:
    db = OmegaConf.create(context.bot_data["db"])
    with open(PATH_DB, "w") as fp:
        OmegaConf.save(config=db, f=fp)
        logger.info("Saved database to file")


async def post_init(application: Application) -> None:
    if PATH_DB.exists():
        with open(PATH_DB, "r") as fp:
            db = OmegaConf.load(fp)
        application.bot_data["db"] = db
    else:
        db = dict()
        application.bot_data["db"] = db
    logger.info("Loaded database:")
    logger.info(application.bot_data["db"])

    # Save db regularly
    application.job_queue.run_repeating(job_save_db, interval=DB_SAVE_INTERVAL,
                                        first=1, name="service_save_db",
                                        job_kwargs={"misfire_grace_time":
                                                    JOB_GRACE_TIME})
    # Start query jobs
    for user_id, data in application.bot_data["db"].items():
        chat_id = data["chat_id"]
        for q in data["qs"]:
            await schedule_query(job_queue=application.job_queue,
                                 user_id=user_id, chat_id=chat_id, query=q)
        await application.bot.send_message(chat_id, text="\U0001F44C Bot relaunched")


async def post_stop(application: Application) -> None:
    for j in application.job_queue.jobs():
        j.stop()
    db = OmegaConf.create(application.bot_data["db"])
    with open(PATH_DB, "w") as fp:
        OmegaConf.save(config=db, f=fp)
        logger.info("Saved database to file")


async def get_news(context: ContextTypes.DEFAULT_TYPE) -> None:
    def _dtime_to_str(d):
        return datetime.strftime(d, "%d.%m.%Y %H:%M")

    def _str_to_dtime(s):
        return datetime.strptime(s, "%d.%m.%Y %H:%M")

    user_id = context.job.user_id
    chat_id = context.job.chat_id
    query = context.job.data

    qs = context.bot_data["db"][user_id]["qs"]
    dtimes = context.bot_data["db"][user_id]["dtimes_prev_req"]
    i = qs.index(query)

    dtime_curr_req = datetime.now()

    ret = requests.get("https://muusikoiden.net/tori/haku.php",
                       params=f"keyword={query}")
    soup = bs4.BeautifulSoup(ret.text, "html.parser")
    ad_hdrs_raw = soup.select('tr[class="bg2"]')

    ads = []
    for ad_hdr_raw in ad_hdrs_raw:
        try:
            ad = dict()

            ad_hdr_l = ad_hdr_raw.select('td[class="tori_title"]')[0].a  # IndexError here
            ad["title"] = ad_hdr_l.text
            url_rel = ad_hdr_l["href"]
            ad["url"] = f"https://muusikoiden.net{url_rel}"

            ad_hdr_r = ad_hdr_raw.select('small[class="light"]')[0]
            timestamps = ad_hdr_r.span["title"]
            patt_added = re.compile(r"\s*Lisätty:\s+([\d\.]+)\s+(\d{2}:\d{2}).*")
            m = patt_added.findall(timestamps)[0]
            ad["dtime_add"] = _str_to_dtime(f"{m[0]} {m[1]}")
            patt_upd = re.compile(r".*Muokattu:\s+([\d\.]+)\s+(\d{2}:\d{2}).*")
            m = patt_upd.findall(timestamps)[0]
            ad["dtime_upd"] = _str_to_dtime(f"{m[0]} {m[1]}")
            ads.append(ad)
        except IndexError:
            pass
    
    if dtimes[i] == "never":
        dtime_prev_req = dtimes[i]
    else:
        dtime_prev_req = _str_to_dtime(dtimes[i])
    for ad in ads:
        if (dtime_prev_req == "never") or (ad["dtime_upd"] >= dtime_prev_req):
            msg = (f"_From query '{query}':_"
                   f"\n*{ad['title']}*"
                   f"\n{ad['url']}"
                   f"\n{ad['dtime_upd']}")
            await context.bot.send_message(chat_id, text=msg, parse_mode="Markdown")
    context.bot_data["db"][user_id]["dtimes_prev_req"][i] = _dtime_to_str(dtime_curr_req)


async def schedule_query(job_queue, user_id: str, chat_id: str, query: str) -> None:
    """Add a job to the queue."""
    job_name = f"{user_id}__{query}"
    job_queue.run_repeating(get_news, interval=QUERY_EXEC_INTERVAL, first=1,
                            data=query, name=job_name,
                            user_id=user_id, chat_id=chat_id,
                            job_kwargs={"misfire_grace_time": JOB_GRACE_TIME})
    msg = f"Job added: {job_name}"
    logger.info(msg)


async def cancel_query(job_queue, user_id: str, chat_id: str, query: str) -> None:
    """Remove a job from the queue."""
    job_name = f"{user_id}__{query}"
    js = job_queue.get_jobs_by_name(job_name)
    if not js:
        logger.info(f"No such job exists: {job_name}")
    for j in js:
        j.schedule_removal()
        logger.info(f"Job {job_name} has been removed")


def main():
    app = (ApplicationBuilder()
           .token(TG_BOT_TOKEN)
           .post_init(post_init)
           .post_stop(post_stop)
           .build())

    app.add_handler(CommandHandler("start", cb_start))
    app.add_handler(CommandHandler("add", cb_query_add))
    app.add_handler(CommandHandler("remove", cb_query_remove))
    app.add_handler(CommandHandler("list", cb_query_list))
    app.add_handler(CommandHandler("clear", cb_query_clear))
    app.add_handler(CommandHandler("stop", cb_stop))
    app.add_handler(MessageHandler(filters.TEXT & ~filters.COMMAND, cb_msg))

    app.run_polling(poll_interval=TG_POLL_INTERVAL, drop_pending_updates=True)

if __name__ == "__main__":
    main()
