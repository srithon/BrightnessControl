# 1.6.0-alpha-0
* Daemon now processes input asynchronously
  * process multiple inputs at the same time
* Fades are now interruptible
  * `brightness_control b -t`
  * can use this in conjunction with increment/decrement/set to replace the current fade
    * `brightness_control b -ti10`
* Improved CLI input validation
  * arguments/flags that should not be used together now explicitly conflict
