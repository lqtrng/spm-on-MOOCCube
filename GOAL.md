Find one public dataset that can be map to this schema:
    time, user, video, topics of that video (which really says about the video's content, not just channel/streamer name, this is not relevant, can exist or not)

Divide dataset to 3 set: train 0.7, val 0.15 and test 0.15
Divide train set to sessions, max gap is 20 minutes, If there's many users, divide by users first, use each session as a sequence so that one user -> many sessions -> aggregate all session into one sequence database, and topics of each video as an itemset.
Use PrefixSpan algorithm to find frequent subsequence patterns (not maximal pattern) with max length 7

Make a topic recommendation system, whose input is current session's video, then uses simple probability based on frequen patterns to choose next topic. 

The way you compute the score: divide by user, then divide by session, for each session, create queries by for each point: use the nearest x video (x is max length pattern) and the next video as the ground truth, aggregate all pair (query, ground truth) into a test set.

The baseline is:
- always predict topic with most video
- always predict the last topic

Goal: don't stop tuning until the score is 10 score higher than the baseline where the increasing boils down to the mined patterns. You can change the dataset, max length pattern, simple probability model. After that, build a demo in one single self contained .html file, that let I choose up to 6 video topic and it will show me the predicted topic, should be easy to use by just loading mined patterns.

Write in rust, make new copy when decided to try other datasets instead of overwrite. if storage go below 5 gb you can move all dataset to 1tb-ssd/model instead